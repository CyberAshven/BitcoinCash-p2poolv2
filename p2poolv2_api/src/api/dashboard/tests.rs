use super::*;
use crate::api::server::{AppConfig, WebOptions, build_router_with_history};
use axum::body::Body;
use axum::http::{HeaderMap, Request, header};
use p2poolv2_lib::accounting::payout::simple_pplns::SimplePplnsShare;
use p2poolv2_lib::accounting::stats::metrics;
use p2poolv2_lib::accounting::stats::user::User;
use p2poolv2_lib::accounting::stats::worker::Worker;
use p2poolv2_lib::monitoring_events::create_monitoring_event_channel;
use p2poolv2_lib::node::actor::NodeHandle;
use p2poolv2_lib::stratum::work::tracker::start_tracker_actor;
use p2poolv2_lib::test_utils::{
    PUBKEY_G, TestShareBlockBuilder, build_confirmed_test_chain, genesis_for_tests,
    setup_test_chain_store_handle,
};
use tower::ServiceExt;

async fn build_state(
    auth: Option<(&str, &str)>,
    cash_network: Option<BchShareNetwork>,
) -> (Arc<AppState>, tempfile::TempDir, tempfile::TempDir) {
    let (chain_store_handle, store_dir) = setup_test_chain_store_handle(true).await;
    let metrics_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(metrics_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    // The middleware expects the stored token in the salt$hmac form that
    // config.toml carries; the tests hand over the login password itself.
    let (auth_user, auth_token) = match auth {
        Some((user, password)) => {
            let salt = "dash-test-salt";
            let hmac =
                p2poolv2_lib::auth::password_to_hmac(salt, password).expect("hmac to compute");
            (Some(user.to_string()), Some(format!("{salt}${hmac}")))
        }
        None => (None, None),
    };
    let state = Arc::new(AppState {
        app_config: AppConfig {
            pool_signature_length: 0,
            network: bitcoin::Network::Signet,
            cash_network,
            cors_allowed: false,
        },
        chain_store_handle,
        metrics_handle,
        tracker_handle: start_tracker_actor(),
        node_handle: NodeHandle::new_for_test(),
        monitoring_event_sender: create_monitoring_event_channel(),
        auth_user,
        auth_token,
        pplns_window: None,
    });
    (state, store_dir, metrics_dir)
}

async fn get(state: &Arc<AppState>, uri: &str) -> (StatusCode, String, HeaderMap) {
    get_with(state, uri, WebOptions::default()).await
}

/// Confirm `share` on top of the chain, as the organise worker would.
async fn confirm_share(state: &AppState, share: &p2poolv2_lib::shares::share_block::ShareBlock) {
    state
        .chain_store_handle
        .add_share_block_and_organise_header(share.clone())
        .await
        .unwrap();
    state
        .chain_store_handle
        .mark_block_valid(share.block_hash())
        .await
        .unwrap();
    state.chain_store_handle.organise_block().await.unwrap();
}

fn recent_share_row_heights(body: &str) -> Vec<u32> {
    let table = body
        .split_once("<h2>Shares")
        .and_then(|(_, rest)| rest.split_once("</table>"))
        .map(|(table, _)| table)
        .expect("overview has a Shares table");
    table
        .split("<tr")
        .skip(1)
        .filter_map(|row| {
            let cell = row.split_once("<td>").map(|(_, cell)| cell)?;
            let height = cell.split_once("</td>").map(|(height, _)| height)?;
            height.parse::<u32>().ok()
        })
        .collect()
}

/// For each row of the Shares table, whether it renders as an uncle, in
/// the same order as `recent_share_row_heights`.
fn recent_share_row_is_uncle(body: &str) -> Vec<bool> {
    let table = body
        .split_once("<h2>Shares")
        .and_then(|(_, rest)| rest.split_once("</table>"))
        .map(|(table, _)| table)
        .expect("overview has a Shares table");
    table
        .split("<tr")
        .skip(1)
        .filter(|row| row.contains("<td"))
        .map(|row| row.contains("class=\"uncle-row\""))
        .collect()
}

async fn get_with(
    state: &Arc<AppState>,
    uri: &str,
    web: WebOptions,
) -> (StatusCode, String, HeaderMap) {
    get_with_headers(state, uri, web, &[]).await
}

async fn get_with_headers(
    state: &Arc<AppState>,
    uri: &str,
    web: WebOptions,
    headers: &[(&str, &str)],
) -> (StatusCode, String, HeaderMap) {
    let app = build_router_with_history(
        Arc::clone(state),
        state.app_config.clone(),
        Arc::new(HashrateHistory::default()),
        web,
    );
    let mut builder = Request::builder().uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = app
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        String::from_utf8_lossy(&bytes).into_owned(),
        headers,
    )
}

fn public() -> WebOptions {
    WebOptions {
        public: true,
        web_root: None,
    }
}

/// A frontend build: an index page and a WebAssembly module.
fn frontend() -> tempfile::TempDir {
    let web_root = tempfile::tempdir().unwrap();
    std::fs::write(web_root.path().join("index.html"), "<p>frontend</p>").unwrap();
    std::fs::write(web_root.path().join("app.wasm"), b"\0asm").unwrap();
    web_root
}

#[tokio::test]
async fn the_overview_is_html_without_script() {
    let (state, _store, _metrics) = build_state(None, Some(BchShareNetwork::Chipnet)).await;
    let (status, body, _) = get(&state, "/dashboard").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!DOCTYPE html>"));
    assert!(body.contains("Bitcoin Cash chipnet"));
    assert!(body.contains("Share chain height"));
    // The Blocks found tile carries the effort note (a dash here: no
    // template has been built yet, so there is no difficulty to divide by).
    assert!(body.contains("Effort —"));
    assert!(!body.contains("<script"));
}

#[tokio::test]
async fn the_overview_lists_the_recent_shares_newest_first() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let genesis_block = genesis_for_tests();
    state
        .chain_store_handle
        .init_or_setup_genesis(genesis_block.clone())
        .await
        .unwrap();
    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis_block.block_hash().to_string())
        .miner_pubkey(PUBKEY_G)
        .build();
    confirm_share(&state, &share).await;
    let (status, body, _) = get(&state, "/dashboard").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recent_share_row_heights(&body), vec![1, 0]);
}

#[tokio::test]
async fn a_known_share_has_a_page() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let genesis_block = genesis_for_tests();
    let genesis = genesis_block.block_hash();
    state
        .chain_store_handle
        .init_or_setup_genesis(genesis_block)
        .await
        .unwrap();
    let (status, body, _) = get(&state, &format!("/dashboard/share/{genesis}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(&genesis.to_string()));
    assert!(body.contains("Bitcoin Cash header"));
}

#[tokio::test]
async fn a_bad_share_hash_is_a_400_page() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let (status, body, _) = get(&state, "/dashboard/share/not-a-hash").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("Not a share hash"));
}

#[tokio::test]
async fn an_unknown_share_is_a_404_page() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let missing = "00".repeat(32);
    let (status, _, _) = get(&state, &format!("/dashboard/share/{missing}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_miner_page_needs_an_address() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let (status, body, _) = get(&state, "/dashboard/miner?address=").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("Enter a payout or share chain address."));
}

/// The stratum-session rates the dashboards read: recorded through the
/// handle, published on the actor's tick, and the miner detail figure and
/// the column read one snapshot, so the two can never disagree.
#[tokio::test]
async fn the_miner_page_hashrate_matches_the_miners_page_column() {
    let (state, _store, _metrics) = build_state(None, None).await;
    for rig in ["rig1", "rig2"] {
        state
            .metrics_handle
            .increment_worker_count("bchtest:q".to_string(), rig.to_string())
            .await
            .expect("authorize worker");
    }
    for rig in ["rig1", "rig1", "rig2"] {
        let share = SimplePplnsShare {
            user_id: 1,
            difficulty: 10,
            btcaddress: Some("bchtest:q".to_string()),
            workername: Some(rig.to_string()),
            n_time: 1_000,
            job_id: "job".to_string(),
            extranonce2: "extra".to_string(),
            nonce: "nonce".to_string(),
        };
        state
            .metrics_handle
            .record_share_accepted(share, 110)
            .await
            .expect("record share");
    }
    // Wait out the actor's publish tick: its interval fires every five
    // seconds, so six seconds guarantee a tick at least a second after the
    // last submission, and the window has a span to divide by.
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;

    let rates = state.metrics_handle.node_rates();
    let stat = rates
        .miners
        .get("bchtest:q")
        .copied()
        .expect("the window holds the miner's submissions");
    assert_eq!(stat.count, 3, "all three submissions are in the window");
    assert!(stat.hashrate > 0.0, "{}", stat.hashrate);
    let hashrate = stat.hashrate;
    let listed = miners_page(&state, local_miners(&state).await, None)
        .await
        .0
        .into_iter()
        .find(|miner| miner.address == "bchtest:q")
        .expect("the miner is listed");
    assert_eq!(
        listed.hashrate.expect("the column shows a figure"),
        hashrate,
        "the miner page and the miners page column read one snapshot"
    );
    // Each worker carries its own figure from the same snapshot.
    for rig in ["rig1", "rig2"] {
        let worker = listed
            .workers
            .iter()
            .find(|worker| worker.name == rig)
            .expect("the worker is listed");
        assert!(
            worker.hashrate.is_some_and(|rate| rate > 0.0),
            "{rig} has its own window"
        );
    }
}

/// A miner that never submitted on this node has no rate anywhere: the
/// detail page renders a dash and the column dashes the same miner.
#[tokio::test]
async fn a_miner_without_submissions_has_no_hashrate() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let stranger = "bchtest:qstrangerwithnosubmissiononthisnode";
    let rates = state.metrics_handle.node_rates();
    assert!(
        !rates.miners.contains_key(stranger),
        "the miner page renders a dash, not 0.00 H/s"
    );
    let listed = miners_page(&state, local_miners(&state).await, None)
        .await
        .0
        .into_iter()
        .find(|miner| miner.address == stranger);
    if let Some(miner) = listed {
        assert_eq!(miner.hashrate, None, "the column dashes the same miner");
    }
    assert!(
        rates.stat.is_none(),
        "the node's own figure is a dash before its first submission"
    );
}

#[tokio::test]
async fn a_first_visit_gets_a_bare_401() {
    let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
    let (status, _, headers) = get(&state, "/dashboard").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!headers.contains_key(header::WWW_AUTHENTICATE));
}

#[tokio::test]
async fn a_wrong_login_gets_a_basic_challenge() {
    let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
    let (status, _, headers) = get_with_headers(
        &state,
        "/dashboard",
        WebOptions::default(),
        &[("authorization", "Basic b3BlcmF0b3I6d3JvbmctcGFzc3dvcmQ=")],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let challenge = headers.get(header::WWW_AUTHENTICATE).unwrap();
    assert!(challenge.to_str().unwrap().starts_with("Basic"));
}

#[tokio::test]
async fn the_wasm_apps_marked_probe_gets_no_challenge() {
    let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
    let (status, _, headers) = get_with_headers(
        &state,
        "/metrics",
        WebOptions::default(),
        &[
            ("authorization", "Basic b3BlcmF0b3I6d3JvbmctcGFzc3dvcmQ="),
            ("x-requested-with", "XMLHttpRequest"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!headers.contains_key(header::WWW_AUTHENTICATE));
}

#[tokio::test]
async fn a_public_node_opens_its_share_chain_but_not_its_own_routes() {
    let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
    let (status, body, _) = get_with(&state, "/dashboard", public()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Share chain height"));
    // A bad address is the handler's 400, not the login's 401.
    let (status, _, _) = get_with(&state, "/miner?address=not-an-address", public()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // The peer list opens beside the share chain, not with the node's own routes.
    let (status, _, _) = get_with(&state, "/peers", public()).await;
    assert_eq!(status, StatusCode::OK);
    for own_route in [
        "/metrics",
        "/blocked_ips",
        "/jd_prepare",
        "/dashboard/signin",
    ] {
        let (status, _, headers) = get_with(&state, own_route, public()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{own_route}");
        assert!(
            !headers.contains_key(header::WWW_AUTHENTICATE),
            "{own_route}"
        );
    }
}

#[tokio::test]
async fn the_admin_signin_page_answers_the_api_login() {
    use base64::Engine;
    let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
    let (status, _, headers) = get_with(&state, "/dashboard/signin", public()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!headers.contains_key(header::WWW_AUTHENTICATE));
    let credentials = base64::engine::general_purpose::STANDARD.encode("operator:secret-token");
    let (status, body, _) = get_with_headers(
        &state,
        "/dashboard/signin",
        public(),
        &[("authorization", &format!("Basic {credentials}"))],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Signed in"));
    assert!(body.contains("/metrics"));
    assert!(!body.contains("<script"));
}

#[tokio::test]
async fn the_root_opens_the_dashboard_without_a_frontend() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let (status, _, headers) = get(&state, "/").await;
    assert!(status.is_redirection());
    assert_eq!(headers.get(header::LOCATION).unwrap(), "/dashboard");
}

#[tokio::test]
async fn a_web_root_serves_its_frontend_beside_the_api() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let web_root = frontend();
    let web = || WebOptions {
        public: false,
        web_root: Some(web_root.path().to_path_buf()),
    };
    let (status, body, _) = get_with(&state, "/", web()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "<p>frontend</p>");
    let (status, _, headers) = get_with(&state, "/app.wasm", web()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/wasm"
    );
    // A path of the frontend's own routing gets its index page.
    let (status, body, _) = get_with(&state, "/miners/someone", web()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "<p>frontend</p>");
    // The API and the built-in dashboard keep their paths.
    let (_, body, _) = get_with(&state, "/health", web()).await;
    assert_eq!(body, "OK");
    let (_, body, _) = get_with(&state, "/dashboard", web()).await;
    assert!(body.contains("Share chain height"));
}

#[tokio::test]
async fn a_web_root_follows_the_login() {
    let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
    let web_root = frontend();
    let web = |public| WebOptions {
        public,
        web_root: Some(web_root.path().to_path_buf()),
    };
    let (status, _, headers) = get_with(&state, "/", web(false)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!headers.contains_key(header::WWW_AUTHENTICATE));
    let (status, body, _) = get_with(&state, "/", web(true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "<p>frontend</p>");
}

#[tokio::test]
async fn the_old_static_files_are_gone() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let (status, _, _) = get(&state, "/static/dashboard.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn every_page_has_the_menu_and_no_script() {
    let (state, _store, _metrics) = build_state(None, Some(BchShareNetwork::Chipnet)).await;
    for path in [
        "/dashboard",
        "/dashboard/blocks",
        "/dashboard/miners",
        "/dashboard/help",
    ] {
        let (status, body, _) = get(&state, path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(body.contains("class=\"site-header\""), "{path}");
        assert!(body.contains("action=\"/dashboard/search\""), "{path}");
        assert!(body.contains("aria-current=\"page\""), "{path}");
        assert!(!body.contains("<script"), "{path}");
    }
}

#[tokio::test]
async fn the_assets_are_served_from_the_binary() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let (status, body, headers) = get(&state, "/dashboard/assets/dashboard.css").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/css")
    );
    assert!(body.contains(".kpi-card"));
    let (status, _, _) = get(&state, "/dashboard/assets/pico.min.css").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = get(&state, "/dashboard/assets/dashboard.js").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn search_opens_a_share_or_a_miner() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let genesis_block = genesis_for_tests();
    let genesis = genesis_block.block_hash();
    state
        .chain_store_handle
        .init_or_setup_genesis(genesis_block)
        .await
        .unwrap();
    let (status, _, headers) = get(&state, &format!("/dashboard/search?q={genesis}")).await;
    assert!(status.is_redirection());
    assert_eq!(
        headers.get(header::LOCATION).unwrap(),
        &format!("/dashboard/share/{genesis}")
    );
    let (status, _, headers) = get(&state, "/dashboard/search?q=+tb1qexample+").await;
    assert!(status.is_redirection());
    assert_eq!(
        headers.get(header::LOCATION).unwrap(),
        "/dashboard/miner?address=tb1qexample"
    );
    let (status, body, _) = get(&state, "/dashboard/search?q=").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("Enter a miner address"));
    let missing = "11".repeat(32);
    let (status, _, _) = get(&state, &format!("/dashboard/search?q={missing}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_share_page_leads_back_to_its_miner() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let genesis_block = genesis_for_tests();
    let genesis = genesis_block.block_hash();
    state
        .chain_store_handle
        .init_or_setup_genesis(genesis_block)
        .await
        .unwrap();
    let (_, body, _) = get(&state, &format!("/dashboard/share/{genesis}")).await;
    assert!(body.contains("aria-label=\"breadcrumb\""));
    assert!(body.contains("href=\"/dashboard/miner?address="));
    assert!(body.contains("href=\"/dashboard/miners\""));
}

#[tokio::test]
async fn an_unknown_block_is_a_404_page() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let (status, body, _) = get(&state, &format!("/dashboard/block/{}", "22".repeat(32))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("has not found a block"));
    let (status, _, _) = get(&state, "/dashboard/block/nope").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_network_is_named_as_the_header_names_it() {
    let (state, _store, _metrics) = build_state(None, Some(BchShareNetwork::Chipnet)).await;
    let (status, body, _) = get(&state, "/network").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        r#"{"network":"Bitcoin Cash chipnet","badge":"chipnet","auth":false}"#
    );
}

#[tokio::test]
async fn the_miners_page_is_served_as_json() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let (status, body, _) = get(&state, "/miners?page=3").await;
    assert_eq!(status, StatusCode::OK);
    let miners: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(miners["miners_online"], 0);
    assert_eq!(miners["workers_online"], 0);
    assert_eq!(miners["total"], 0);
    assert_eq!(miners["page"], 1);
    assert_eq!(miners["pages"], 1);
    assert_eq!(miners["page_size"], PAGE_SIZE);
    assert_eq!(miners["accepted_shares_total"], 0);
    assert_eq!(miners["rejected_shares_total"], 0);
    assert_eq!(miners["miners"], serde_json::json!([]));
}

#[tokio::test]
async fn the_miners_json_carries_the_pools_share_totals() {
    let (state, _store, _metrics) = build_state(None, None).await;
    state
        .metrics_handle
        .increment_worker_count("bchtest:qrt".to_string(), "rig".to_string())
        .await
        .expect("authorize worker");
    let share = SimplePplnsShare {
        user_id: 1,
        difficulty: 100,
        btcaddress: Some("bchtest:qrt".to_string()),
        workername: Some("rig".to_string()),
        n_time: 1_000,
        job_id: "job".to_string(),
        extranonce2: "extra".to_string(),
        nonce: "nonce".to_string(),
    };
    state
        .metrics_handle
        .record_share_accepted(share, 110)
        .await
        .expect("record share");
    state
        .metrics_handle
        .record_share_rejected()
        .await
        .expect("record rejection");
    let (status, body, _) = get(&state, "/miners").await;
    assert_eq!(status, StatusCode::OK);
    let miners: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(miners["accepted_shares_total"], 1);
    assert_eq!(miners["rejected_shares_total"], 1);
}

#[tokio::test]
async fn a_public_node_opens_the_miners_and_the_network() {
    let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
    let (status, _, _) = get_with(&state, "/miners", public()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = get_with(&state, "/network", public()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = get(&state, "/miners").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[test]
fn a_miner_row_serializes_as_the_wasm_app_reads_it() {
    // p2poolv2_web's `LocalMiner` and `Worker` decode these names.
    let row = MinerRow {
        address: "bchtest:q".to_string(),
        workers: vec![WorkerRow {
            name: "rig".to_string(),
            active: true,
            accepted_work: 10,
            best_share: 5,
            last_share_at: 9,
            hashrate: Some(2.0),
        }],
        accepted_work: 10,
        best_share: 5,
        last_share_at: 9,
        payout_fraction: None,
        hashrate: None,
    };
    assert_eq!(
        serde_json::to_value(&row).unwrap(),
        serde_json::json!({
            "address": "bchtest:q",
            "workers": [{
                "name": "rig",
                "active": true,
                "accepted_work": 10,
                "best_share": 5,
                "last_share_at": 9,
                "hashrate": 2.0
            }],
            "accepted_work": 10,
            "best_share": 5,
            "last_share_at": 9,
            "payout_fraction": null,
            "hashrate": null
        })
    );
}

#[test]
fn online_counts_count_active_workers() {
    let miner = |active: &[bool]| MinerRow {
        address: "bchtest:q".to_string(),
        workers: active
            .iter()
            .map(|active| WorkerRow {
                name: "rig".to_string(),
                active: *active,
                accepted_work: 1,
                best_share: 1,
                last_share_at: 1,
                hashrate: None,
            })
            .collect(),
        accepted_work: 1,
        best_share: 1,
        last_share_at: 1,
        payout_fraction: None,
        hashrate: None,
    };
    let miners = [miner(&[true, false, true]), miner(&[false]), miner(&[])];
    assert_eq!(online_counts(&miners), (1, 2));
}

#[test]
fn online_counts_from_metrics_counts_the_same_way_as_online_counts() {
    // One miner with two workers, one of them active, and one miner whose
    // only worker is idle: the metrics-side counts must read (1, 1) as
    // the row-based counts do.
    let active_worker = Worker {
        active: true,
        ..Worker::default()
    };
    let idle_worker = Worker {
        active: false,
        ..Worker::default()
    };
    let mut busy_user = User::default();
    busy_user.workers.insert("rig-a".to_string(), active_worker);
    busy_user.workers.insert("rig-b".to_string(), idle_worker);
    let mut quiet_user = User::default();
    quiet_user
        .workers
        .insert("rig-c".to_string(), Worker::default());
    let mut metrics = metrics::PoolMetrics::default();
    metrics.users.insert("miner1".to_string(), busy_user);
    metrics.users.insert("miner2".to_string(), quiet_user);

    // The same two miners as rows: the first with an active and an idle
    // worker, the second with only an idle one.
    let miner = |active: &[bool]| MinerRow {
        address: "bchtest:q".to_string(),
        workers: active
            .iter()
            .enumerate()
            .map(|(index, active)| WorkerRow {
                name: format!("rig-{index}"),
                active: *active,
                accepted_work: 1,
                best_share: 1,
                last_share_at: 1,
                hashrate: None,
            })
            .collect(),
        accepted_work: 1,
        best_share: 1,
        last_share_at: 1,
        payout_fraction: None,
        hashrate: None,
    };
    let rows = [miner(&[true, false]), miner(&[false])];
    assert_eq!(online_counts(&rows), (1, 1));
    assert_eq!(online_counts_from_metrics(&metrics), online_counts(&rows));
}

#[test]
fn a_looked_up_miner_matches_either_spelling() {
    let miner = MinerRow {
        address: "qpayout".to_string(),
        workers: Vec::new(),
        accepted_work: 0,
        best_share: 0,
        last_share_at: 0,
        payout_fraction: None,
        hashrate: None,
    };
    assert!(is_looked_up_miner(&miner, "qpayout", None));
    assert!(is_looked_up_miner(
        &miner,
        "bchtest:qpayout",
        Some("qpayout")
    ));
    assert!(!is_looked_up_miner(
        &miner,
        "bchtest:qpayout",
        Some("bchtest:qpayout")
    ));
}

#[tokio::test]
async fn the_overview_recent_shares_stop_at_five_heights() {
    let (state, _store, _metrics) = build_state(None, None).await;
    build_confirmed_test_chain(&state.chain_store_handle, 30).await;
    let (status, body, _) = get(&state, "/dashboard").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        recent_share_row_heights(&body),
        vec![30, 29, 28, 27, 26],
        "the overview lists the five newest heights, newest first"
    );
}

#[tokio::test]
async fn the_overview_recent_shares_hold_five_rows_when_a_share_carries_an_uncle() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let genesis_block = genesis_for_tests();
    state
        .chain_store_handle
        .init_or_setup_genesis(genesis_block.clone())
        .await
        .unwrap();
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis_block.block_hash().to_string())
        .build();
    confirm_share(&state, &share1).await;
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .build();
    confirm_share(&state, &share2).await;
    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .build();
    confirm_share(&state, &share3).await;
    // An off-chain sibling of share3: share4 names it as an uncle, the
    // shape a race between two miners leaves behind. Its nonce differs so
    // it does not hash to the same block as share3, its sibling.
    let uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .nonce(0xe9695799)
        .build();
    confirm_share(&state, &uncle).await;
    let share4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share3.block_hash().to_string())
        .uncles(vec![uncle.block_hash()])
        .build();
    confirm_share(&state, &share4).await;
    let share5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share4.block_hash().to_string())
        .build();
    confirm_share(&state, &share5).await;
    let share6 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share5.block_hash().to_string())
        .build();
    confirm_share(&state, &share6).await;
    let share7 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share6.block_hash().to_string())
        .build();
    confirm_share(&state, &share7).await;

    let (status, body, _) = get(&state, "/dashboard").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        recent_share_row_heights(&body),
        vec![7, 6, 5, 4, 3],
        "the overview holds five rows, dropping share3's row to keep the uncle under share4"
    );
    assert_eq!(
        recent_share_row_is_uncle(&body),
        vec![false, false, false, false, true],
        "the kept fifth row is the uncle, directly under the share that includes it"
    );
    assert_eq!(
        body.matches("class=\"uncle-row\"").count(),
        1,
        "the overview renders the uncle once"
    );

    let (status, body, _) = get(&state, "/dashboard/shares").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        recent_share_row_heights(&body),
        vec![7, 6, 5, 4, 3, 3, 2, 1, 0],
        "the shares page lists every height, the uncle under the share that includes it"
    );
    assert_eq!(
        recent_share_row_is_uncle(&body),
        vec![false, false, false, false, true, false, false, false, false],
        "the shares page keeps the uncle between share4 and share3"
    );
}

#[tokio::test]
async fn the_peers_page_is_served() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let (status, body, _) = get(&state, "/dashboard/peers").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!DOCTYPE html>"));
    assert!(body.contains("<h2>Peers"));
    assert!(!body.contains("<script"));
}

#[tokio::test]
async fn the_overview_links_to_the_shares_page_instead_of_a_pager() {
    let (state, _store, _metrics) = build_state(None, None).await;
    build_confirmed_test_chain(&state.chain_store_handle, 8).await;
    let (status, body, _) = get(&state, "/dashboard").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("href=\"/dashboard/shares\""));
    assert!(body.contains("More found shares"));
    assert!(!body.contains("page="));
    assert!(!body.contains("rel=\"prev\""));
}

#[tokio::test]
async fn the_overview_ignores_its_page_parameter() {
    let (state, _store, _metrics) = build_state(None, None).await;
    build_confirmed_test_chain(&state.chain_store_handle, 8).await;
    let (_, plain, _) = get(&state, "/dashboard").await;
    let (status, paged, _) = get(&state, "/dashboard?page=999").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        recent_share_row_heights(&paged),
        recent_share_row_heights(&plain),
        "the overview shows the same recent shares on every request"
    );
    assert_eq!(recent_share_row_heights(&paged), vec![8, 7, 6, 5, 4]);
}

#[tokio::test]
async fn the_shares_page_paces_the_whole_chain() {
    let (state, _store, _metrics) = build_state(None, None).await;
    build_confirmed_test_chain(&state.chain_store_handle, 30).await;
    let (status, body, _) = get(&state, "/dashboard/shares").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        recent_share_row_heights(&body),
        (6..=30).rev().collect::<Vec<u32>>(),
        "the first page holds twenty-five heights, newest first"
    );
    assert!(body.contains("Page 1 of 2"));
    assert!(body.contains("/dashboard/shares?page=2"));
    let (_, second, _) = get(&state, "/dashboard/shares?page=2").await;
    assert_eq!(
        recent_share_row_heights(&second),
        vec![5, 4, 3, 2, 1, 0],
        "the second page carries the rest down to genesis"
    );
    assert!(second.contains("Page 2 of 2"));
}

#[tokio::test]
async fn the_shares_page_without_a_chain_says_so() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let (status, body, _) = get(&state, "/dashboard/shares").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("No confirmed shares yet."));
    assert!(!body.contains("rel=\"prev\""));
}

#[tokio::test]
async fn the_series_toggles_read_their_query_parameters() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let (status, body, _) = get(&state, "/dashboard?pool=0").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("class=\"legend-chip off\""));
    assert!(body.contains("aria-pressed=\"false\""));
    // The pool chip is off: its link turns it back on by omitting the
    // parameter, the node chip still flips node.
    assert!(body.contains("/dashboard?range=24h\">"));
    assert!(body.contains("/dashboard?range=24h&amp;pool=0&amp;node=0\""));
}

/// The genesis share has no parent, so its page links to none: the all-zero
/// hash a genesis header carries is no share, and a link to it is a dead one.
#[tokio::test]
async fn the_genesis_share_page_links_to_no_parent() {
    let (state, _store, _metrics) = build_state(None, None).await;
    let genesis_block = genesis_for_tests();
    state
        .chain_store_handle
        .init_or_setup_genesis(genesis_block.clone())
        .await
        .unwrap();

    let (status, body, _) = get(
        &state,
        &format!("/dashboard/share/{}", genesis_block.block_hash()),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains(&format!("/dashboard/share/{}", "0".repeat(64))),
        "the genesis share page links to no parent: {body}"
    );
}

mod browser;
