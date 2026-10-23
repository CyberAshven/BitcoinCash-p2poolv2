// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The application state, ported from PDM's `App`.
//!
//! Fetches run on the tokio runtime and send their answers back through one
//! bounded channel; the screen drains it before each frame. Each source has
//! at most one request in flight and is asked again every
//! [`REFRESH_INTERVAL`] while its screen is shown, so a slow node never
//! piles up requests. Answers carry the generation they were asked in, and
//! loading another node config starts a new one, so a late answer from the
//! old node is dropped. The live feed has its own bounded channel and a
//! task that is aborted when the node config changes. Every list kept is
//! capped.

use crate::client::api::ApiClient;
use crate::client::bchn::BchnClient;
use crate::client::http::display_url;
use crate::client::live::{self, LiveMessage};
use crate::config_editor::ConfigEditor;
use crate::explorer::FileExplorer;
use crate::model::{
    BchnStatus, ChainInfo, FoundBlock, Hashrate, LiveEvent, MAX_SHARE_CHAIN_PEERS, Miner, Miners,
    Peer, PeerEvent, PeerStatus, Share, Shares, Site,
};
use crate::node_config::{self, NodeConnection};
use crate::remote::Remote;
use crate::settings::{self, Settings};
use crate::views::config::{ConfigRow, ConfigScreen};
use crate::views::home::HomeScreen;
use crate::views::pool::PoolScreen;
use crate::views::settings::SettingsScreen;
use crate::views::{
    Hint, SIDEBAR_ITEMS, Screen, bchn, config, explorer, home, pool, render_shell,
    settings as settings_view,
};
use futures::FutureExt;
use futures::future::BoxFuture;
use ratatui::Frame;
use std::collections::VecDeque;
use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::task::JoinHandle;

/// How often a shown source is asked again.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// How long to wait before opening the live feed again after it ended.
pub const LIVE_RETRY_INTERVAL: Duration = Duration::from_secs(10);

/// Live shares kept, as in PDM.
pub const MAX_LIVE_SHARES: usize = 50;

/// Live peer events kept, as in PDM.
pub const MAX_LIVE_PEER_EVENTS: usize = 50;

/// The longest value typed into the config editor.
pub const MAX_INPUT_CHARS: usize = 1024;

const UPDATE_CHANNEL_CAPACITY: usize = 64;
const LIVE_CHANNEL_CAPACITY: usize = 64;

/// Answers and live messages taken per frame, so a burst cannot hold the
/// screen.
const MAX_MESSAGES_PER_FRAME: usize = 256;

/// What is fetched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Bchn,
    Network,
    ChainInfo,
    Hashrate,
    Shares,
    Miners,
    Miner,
    FoundBlocks,
    Peers,
}

const SOURCE_COUNT: usize = 9;

/// A finished fetch.
#[derive(Debug)]
pub enum Payload {
    Bchn(Result<BchnStatus, String>),
    Network(Result<Site, String>),
    ChainInfo(Result<ChainInfo, String>),
    Hashrate(Result<Hashrate, String>),
    Shares(Result<Shares, String>),
    Miners(Result<Miners, String>),
    Miner(Result<Miner, String>),
    FoundBlocks(Result<Vec<FoundBlock>, String>),
    Peers(Result<Vec<Peer>, String>),
}

impl Payload {
    fn source(&self) -> Source {
        match self {
            Payload::Bchn(_) => Source::Bchn,
            Payload::Network(_) => Source::Network,
            Payload::ChainInfo(_) => Source::ChainInfo,
            Payload::Hashrate(_) => Source::Hashrate,
            Payload::Shares(_) => Source::Shares,
            Payload::Miners(_) => Source::Miners,
            Payload::Miner(_) => Source::Miner,
            Payload::FoundBlocks(_) => Source::FoundBlocks,
            Payload::Peers(_) => Source::Peers,
        }
    }
}

/// A finished fetch and the generation it was asked in.
#[derive(Debug)]
pub struct Update {
    pub generation: u64,
    pub payload: Payload,
}

#[derive(Clone, Copy, Debug, Default)]
struct FetchState {
    in_flight: bool,
    last_started: Option<Instant>,
}

/// Where the file picker was opened from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplorerTrigger {
    PoolConfig,
    /// The settings field with this index.
    Settings(usize),
}

/// The config editor's screen state.
#[derive(Clone, Debug, Default)]
pub struct EditorState {
    pub selected: usize,
    pub focused: bool,
    /// The value being typed, while editing.
    pub input: Option<String>,
    pub warning: Option<String>,
    pub info: Option<String>,
}

/// The settings screen's state.
#[derive(Clone, Debug, Default)]
pub struct SettingsState {
    pub selected: usize,
    pub focused: bool,
    /// Why the settings could not be saved.
    pub error: Option<String>,
}

/// Everything the screen shows and the fetches behind it.
pub struct App {
    pub screen: Screen,
    pub sidebar_index: usize,
    pub explorer: FileExplorer,
    pub explorer_trigger: Option<ExplorerTrigger>,
    pub settings: Settings,
    pub default_settings_dir: PathBuf,
    pub settings_state: SettingsState,
    pub node_config_path: Option<PathBuf>,
    pub connection: Option<NodeConnection>,
    pub connection_error: Option<String>,
    api: Option<ApiClient>,
    bchn: Option<BchnClient>,
    pub editor: Option<ConfigEditor>,
    pub config_rows: Vec<ConfigRow>,
    pub editor_state: EditorState,
    pub bchn_tab: usize,
    pub pool_tab: usize,
    pub bchn_status: Remote<BchnStatus>,
    pub site: Remote<Site>,
    pub chain_info: Remote<ChainInfo>,
    pub hashrate: Remote<Hashrate>,
    pub shares: Remote<Shares>,
    pub miners: Remote<Miners>,
    pub miner: Remote<Miner>,
    pub found_blocks: Remote<Vec<FoundBlock>>,
    pub peers: Remote<Vec<Peer>>,
    /// Oldest first, at most [`MAX_LIVE_SHARES`].
    pub live_shares: VecDeque<Share>,
    /// Oldest first, at most [`MAX_LIVE_PEER_EVENTS`].
    pub live_peer_events: VecDeque<PeerEvent>,
    pub live_error: Option<String>,
    live_task: Option<JoinHandle<()>>,
    live_receiver: Option<mpsc::Receiver<LiveMessage>>,
    live_retry_at: Option<Instant>,
    pub miners_focused: bool,
    pub miners_selected: usize,
    /// The miner whose window place is shown.
    pub miner_address: Option<String>,
    fetches: [FetchState; SOURCE_COUNT],
    generation: u64,
    sender: mpsc::Sender<Update>,
    receiver: mpsc::Receiver<Update>,
    runtime: Handle,
}

impl App {
    /// An app with no node config yet, spawning its fetches on `runtime`.
    pub fn new(settings: Settings, default_settings_dir: PathBuf, runtime: Handle) -> Self {
        let (sender, receiver) = mpsc::channel(UPDATE_CHANNEL_CAPACITY);
        let start_dir = settings
            .node_config_path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            screen: Screen::Home,
            sidebar_index: 0,
            explorer: FileExplorer::new(start_dir, false),
            explorer_trigger: None,
            settings,
            default_settings_dir,
            settings_state: SettingsState::default(),
            node_config_path: None,
            connection: None,
            connection_error: None,
            api: None,
            bchn: None,
            editor: None,
            config_rows: Vec::new(),
            editor_state: EditorState::default(),
            bchn_tab: 0,
            pool_tab: 0,
            bchn_status: Remote::Unconfigured,
            site: Remote::Unconfigured,
            chain_info: Remote::Unconfigured,
            hashrate: Remote::Unconfigured,
            shares: Remote::Unconfigured,
            miners: Remote::Unconfigured,
            miner: Remote::Unconfigured,
            found_blocks: Remote::Unconfigured,
            peers: Remote::Unconfigured,
            live_shares: VecDeque::with_capacity(MAX_LIVE_SHARES),
            live_peer_events: VecDeque::with_capacity(MAX_LIVE_PEER_EVENTS),
            live_error: None,
            live_task: None,
            live_receiver: None,
            live_retry_at: None,
            miners_focused: false,
            miners_selected: 0,
            miner_address: None,
            fetches: [FetchState::default(); SOURCE_COUNT],
            generation: 0,
            sender,
            receiver,
            runtime,
        }
    }

    /// Points the app at the node config at `path`: its API, BCHN and
    /// network, and the editor on the file. Returns whether the file opened
    /// in the editor, which is when it is worth remembering.
    pub fn open_node_config(&mut self, path: PathBuf) -> bool {
        self.reset_remote_state();
        match node_config::load(&path) {
            Ok(connection) => self.connect(connection),
            Err(error) => self.connection_error = Some(error),
        }
        let opened = match ConfigEditor::open(&path) {
            Ok(editor) => {
                self.editor = Some(editor);
                self.editor_state = EditorState::default();
                self.refresh_config_rows();
                true
            }
            Err(error) => {
                self.editor_state = EditorState {
                    warning: Some(error),
                    ..EditorState::default()
                };
                false
            }
        };
        self.explorer = FileExplorer::new(
            path.parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
            false,
        );
        self.node_config_path = Some(path);
        opened
    }

    /// Forgets the node config.
    pub fn clear_node_config(&mut self) {
        self.reset_remote_state();
        self.node_config_path = None;
        self.editor = None;
        self.config_rows.clear();
        self.editor_state = EditorState::default();
    }

    /// Reads the node config again, after it was saved: its API or BCHN
    /// login may have changed.
    pub fn reload_connection(&mut self) {
        let Some(path) = self.node_config_path.clone() else {
            return;
        };
        self.reset_remote_state();
        match node_config::load(&path) {
            Ok(connection) => self.connect(connection),
            Err(error) => self.connection_error = Some(error),
        }
    }

    fn connect(&mut self, connection: NodeConnection) {
        let api = ApiClient::new(&connection.api_base, connection.api_credentials.clone());
        let bchn = BchnClient::new(
            &connection.bchn_url,
            &connection.bchn_username,
            &connection.bchn_password,
        );
        match (api, bchn) {
            (Ok(api), Ok(bchn)) => {
                self.api = Some(api);
                self.bchn = Some(bchn);
                self.connection = Some(connection);
            }
            (Err(error), _) | (_, Err(error)) => self.connection_error = Some(error),
        }
    }

    /// Drops every answer, client and live event of the previous node
    /// config and starts a new generation.
    fn reset_remote_state(&mut self) {
        self.generation += 1;
        self.fetches = [FetchState::default(); SOURCE_COUNT];
        self.stop_live();
        self.live_shares.clear();
        self.live_peer_events.clear();
        self.live_error = None;
        self.live_retry_at = None;
        self.api = None;
        self.bchn = None;
        self.connection = None;
        self.connection_error = None;
        self.bchn_status = Remote::Unconfigured;
        self.site = Remote::Unconfigured;
        self.chain_info = Remote::Unconfigured;
        self.hashrate = Remote::Unconfigured;
        self.shares = Remote::Unconfigured;
        self.miners = Remote::Unconfigured;
        self.miner = Remote::Unconfigured;
        self.found_blocks = Remote::Unconfigured;
        self.peers = Remote::Unconfigured;
        self.miners_focused = false;
        self.miners_selected = 0;
        self.miner_address = None;
    }

    fn stop_live(&mut self) {
        if let Some(task) = self.live_task.take() {
            task.abort();
        }
        self.live_receiver = None;
    }

    pub fn refresh_config_rows(&mut self) {
        self.config_rows = self
            .editor
            .as_ref()
            .map_or_else(Vec::new, ConfigEditor::rows);
    }

    /// The sources the shown screen and tab draw.
    pub fn wanted_sources(&self) -> Vec<Source> {
        let mut sources = Vec::with_capacity(3);
        match (self.screen, self.pool_tab) {
            (Screen::Home, _) => sources.extend([Source::Network, Source::Bchn]),
            (Screen::Bchn, _) => sources.push(Source::Bchn),
            (Screen::PoolStatus, 0) => {
                sources.extend([Source::Network, Source::ChainInfo, Source::Hashrate]);
            }
            (Screen::PoolStatus, 1) => sources.push(Source::Shares),
            (Screen::PoolStatus, 2) => {
                sources.push(Source::Miners);
                if self.miner_address.is_some() {
                    sources.push(Source::Miner);
                }
            }
            (Screen::PoolStatus, 3) => sources.push(Source::FoundBlocks),
            (Screen::PoolStatus, _) => sources.push(Source::Peers),
            (Screen::PoolConfig | Screen::Settings | Screen::FileExplorer, _) => {}
        }
        sources
    }

    /// Starts the fetches that are due and the live feed when the P2Pool
    /// status screen wants it.
    pub fn tick(&mut self, now: Instant) {
        for source in self.wanted_sources() {
            let state = self.fetches[source as usize];
            let due = state
                .last_started
                .is_none_or(|started| now.duration_since(started) >= REFRESH_INTERVAL);
            if due && !state.in_flight {
                self.spawn_fetch(source, now);
            }
        }
        let live_due = self.live_retry_at.is_none_or(|retry_at| now >= retry_at);
        if self.screen == Screen::PoolStatus && self.live_task.is_none() && live_due {
            self.start_live();
        }
    }

    /// Asks the shown screen's sources again now, except those in flight.
    pub fn refresh_now(&mut self) {
        for source in self.wanted_sources() {
            self.fetches[source as usize].last_started = None;
        }
        self.live_retry_at = None;
    }

    /// The request for `source`, `None` when there is nothing to ask.
    fn fetch_for(&self, source: Source) -> Option<BoxFuture<'static, Payload>> {
        let api = self.api.clone();
        match source {
            Source::Bchn => self
                .bchn
                .clone()
                .map(|client| async move { Payload::Bchn(client.status().await) }.boxed()),
            Source::Network => {
                api.map(|api| async move { Payload::Network(api.network().await) }.boxed())
            }
            Source::ChainInfo => {
                api.map(|api| async move { Payload::ChainInfo(api.chain_info().await) }.boxed())
            }
            Source::Hashrate => {
                api.map(|api| async move { Payload::Hashrate(api.hashrate().await) }.boxed())
            }
            Source::Shares => {
                api.map(|api| async move { Payload::Shares(api.shares().await) }.boxed())
            }
            Source::Miners => {
                api.map(|api| async move { Payload::Miners(api.miners().await) }.boxed())
            }
            Source::Miner => api.zip(self.miner_address.clone()).map(|(api, address)| {
                async move { Payload::Miner(api.miner(&address).await) }.boxed()
            }),
            Source::FoundBlocks => {
                api.map(|api| async move { Payload::FoundBlocks(api.found_blocks().await) }.boxed())
            }
            Source::Peers => {
                api.map(|api| async move { Payload::Peers(api.peers().await) }.boxed())
            }
        }
    }

    fn spawn_fetch(&mut self, source: Source, now: Instant) {
        let Some(fetch) = self.fetch_for(source) else {
            return;
        };
        self.fetches[source as usize] = FetchState {
            in_flight: true,
            last_started: Some(now),
        };
        self.mark_loading(source);
        let sender = self.sender.clone();
        let generation = self.generation;
        self.runtime.spawn(async move {
            let payload = fetch.await;
            // A failed send means the app is gone; the answer has nowhere
            // to go.
            let _ = sender
                .send(Update {
                    generation,
                    payload,
                })
                .await;
        });
    }

    /// Shows a first request as loading; a value already shown stays until
    /// its next answer.
    fn mark_loading(&mut self, source: Source) {
        fn loading<T>(remote: &mut Remote<T>) {
            if matches!(remote, Remote::Unconfigured) {
                *remote = Remote::Loading;
            }
        }
        match source {
            Source::Bchn => loading(&mut self.bchn_status),
            Source::Network => loading(&mut self.site),
            Source::ChainInfo => loading(&mut self.chain_info),
            Source::Hashrate => loading(&mut self.hashrate),
            Source::Shares => loading(&mut self.shares),
            Source::Miners => loading(&mut self.miners),
            Source::Miner => loading(&mut self.miner),
            Source::FoundBlocks => loading(&mut self.found_blocks),
            Source::Peers => loading(&mut self.peers),
        }
    }

    fn start_live(&mut self) {
        let Some(api) = &self.api else {
            return;
        };
        let (sender, receiver) = mpsc::channel(LIVE_CHANNEL_CAPACITY);
        self.live_task = Some(self.runtime.spawn(live::run(api.websocket_url(), sender)));
        self.live_receiver = Some(receiver);
    }

    /// Takes what the fetches and the live feed sent since the last frame.
    pub fn drain(&mut self, now: Instant) {
        let mut taken = 0;
        while taken < MAX_MESSAGES_PER_FRAME
            && let Ok(update) = self.receiver.try_recv()
        {
            self.apply(update);
            taken += 1;
        }
        let mut live_pending = true;
        while taken < MAX_MESSAGES_PER_FRAME && live_pending {
            let received = self
                .live_receiver
                .as_mut()
                .map(|receiver| receiver.try_recv());
            match received {
                Some(Ok(message)) => {
                    self.apply_live(message, now);
                    taken += 1;
                }
                // The feed's task is gone without saying why, as when it
                // panicked; treat it as ended, so it is opened again.
                Some(Err(TryRecvError::Disconnected)) => {
                    self.apply_live(LiveMessage::Ended("the live feed stopped".into()), now);
                    live_pending = false;
                }
                Some(Err(TryRecvError::Empty)) | None => live_pending = false,
            }
        }
    }

    /// Applies one finished fetch, unless it was asked for another node
    /// config.
    pub fn apply(&mut self, update: Update) {
        if update.generation != self.generation {
            return;
        }
        self.fetches[update.payload.source() as usize].in_flight = false;
        match update.payload {
            Payload::Bchn(result) => self.bchn_status = Remote::from_result(result),
            Payload::Network(result) => self.site = Remote::from_result(result),
            Payload::ChainInfo(result) => self.chain_info = Remote::from_result(result),
            Payload::Hashrate(result) => self.hashrate = Remote::from_result(result),
            Payload::Shares(result) => self.shares = Remote::from_result(result),
            Payload::Miners(result) => {
                self.miners = Remote::from_result(result);
                let count = self.miners.ready().map_or(0, |miners| miners.miners.len());
                self.miners_selected = self.miners_selected.min(count.saturating_sub(1));
            }
            Payload::Miner(result) => self.miner = Remote::from_result(result),
            Payload::FoundBlocks(result) => self.found_blocks = Remote::from_result(result),
            Payload::Peers(result) => self.peers = Remote::from_result(result),
        }
    }

    /// Applies one live message.
    pub fn apply_live(&mut self, message: LiveMessage, now: Instant) {
        match message {
            LiveMessage::Event(LiveEvent::Share(share)) => {
                push_capped(&mut self.live_shares, share, MAX_LIVE_SHARES);
                self.live_error = None;
            }
            LiveMessage::Event(LiveEvent::Peer(event)) => {
                self.apply_peer_event(&event);
                push_capped(&mut self.live_peer_events, event, MAX_LIVE_PEER_EVENTS);
                self.live_error = None;
            }
            LiveMessage::Malformed(error) => {
                self.live_error = Some(format!("a frame was not understood: {error}"));
            }
            LiveMessage::Ended(reason) => {
                self.live_error = Some(reason);
                self.live_task = None;
                self.live_receiver = None;
                self.live_retry_at = Some(now + LIVE_RETRY_INTERVAL);
            }
        }
    }

    /// PDM's peer update: a peer that left goes, a new one is added by its
    /// id until `/peers` names it fully. Only a list `/peers` gave is
    /// changed.
    fn apply_peer_event(&mut self, event: &PeerEvent) {
        let Remote::Ready(peers) = &mut self.peers else {
            return;
        };
        match event.status {
            PeerStatus::Disconnected => peers.retain(|peer| peer.peer_id != event.peer_id),
            PeerStatus::Connected => {
                let known = peers.iter().any(|peer| peer.peer_id == event.peer_id);
                if !known && peers.len() < MAX_SHARE_CHAIN_PEERS {
                    peers.push(Peer {
                        peer_id: event.peer_id.clone(),
                        address: None,
                        direction: None,
                        connected_secs: None,
                    });
                }
            }
        }
    }

    /// Shows the sidebar's screen at `index`, PDM's `toggle_menu`.
    pub fn select_sidebar(&mut self, index: usize) {
        let Some(&(_, screen)) = SIDEBAR_ITEMS.get(index) else {
            return;
        };
        if self.screen == Screen::PoolConfig && screen != Screen::PoolConfig {
            self.editor_state.input = None;
            self.editor_state.info = None;
        }
        self.sidebar_index = index;
        self.screen = screen;
    }

    /// A value is being typed, so letter keys are text.
    pub fn text_input_active(&self) -> bool {
        self.screen == Screen::PoolConfig && self.editor_state.input.is_some()
    }

    /// Picks the miner under the cursor in the miners table, to show its
    /// place in the PPLNS window.
    pub fn pick_miner(&mut self, now: Instant) {
        let address = self
            .miners
            .ready()
            .and_then(|miners| miners.miners.get(self.miners_selected))
            .map(|miner| miner.address.clone());
        if address.is_some() && address != self.miner_address {
            self.miner_address = address;
            self.miner = Remote::Unconfigured;
            self.fetches[Source::Miner as usize] = FetchState::default();
            self.tick(now);
        }
    }

    /// Commits the typed value to the selected setting.
    pub fn commit_edit(&mut self) {
        let Some(input) = self.editor_state.input.take() else {
            return;
        };
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        self.editor_state.info = None;
        match editor.set(self.editor_state.selected, &input) {
            Ok(true) => self.editor_state.warning = editor.validate().err(),
            Ok(false) => {
                self.editor_state.info =
                    Some("Unchanged: an empty secret keeps the old one.".into());
            }
            Err(error) => self.editor_state.warning = Some(error),
        }
        self.refresh_config_rows();
    }

    /// Saves the config editor's file and reads its connection again.
    pub fn save_config(&mut self) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        match editor.save() {
            Ok(()) => {
                self.editor_state.warning = None;
                self.editor_state.info =
                    Some("Saved. Restart the node for it to take the new settings.".to_string());
                self.reload_connection();
            }
            Err(error) => {
                self.editor_state.info = None;
                self.editor_state.warning = Some(format!("Not saved: {error}"));
            }
        }
    }

    /// Writes the settings, keeping why it failed for the status bar.
    pub fn save_settings(&mut self) {
        self.settings_state.error = settings::save(&self.settings, &self.default_settings_dir)
            .err()
            .map(|error| format!("Save failed: {error}"));
    }

    /// Opens the file picker for `trigger`.
    pub fn open_explorer(&mut self, trigger: ExplorerTrigger, directory_mode: bool) {
        if self.explorer.directory_mode != directory_mode {
            self.explorer.directory_mode = directory_mode;
            self.explorer.load_directory();
        }
        self.explorer_trigger = Some(trigger);
        self.screen = Screen::FileExplorer;
    }

    /// Leaves the file picker for the screen it was opened from.
    pub fn close_explorer(&mut self) {
        self.explorer_trigger = None;
        self.select_sidebar(self.sidebar_index);
    }

    /// Acts on a path picked in the file picker.
    pub fn file_picked(&mut self, path: PathBuf) {
        let Some(trigger) = self.explorer_trigger.take() else {
            return;
        };
        match trigger {
            ExplorerTrigger::PoolConfig | ExplorerTrigger::Settings(0) => {
                if self.open_node_config(path.clone()) {
                    self.settings.node_config_path = Some(path);
                    self.save_settings();
                } else {
                    self.settings_state.error = self.editor_state.warning.clone();
                }
            }
            ExplorerTrigger::Settings(_) => {
                self.settings.settings_dir_override = Some(path);
                self.save_settings();
            }
        }
        self.select_sidebar(self.sidebar_index);
    }

    /// Clears settings field `index`.
    pub fn clear_setting(&mut self, index: usize) {
        match index {
            0 => {
                self.settings.node_config_path = None;
                self.clear_node_config();
            }
            _ => self.settings.settings_dir_override = None,
        }
        self.save_settings();
    }

    /// The sidebar has the keyboard.
    pub fn sidebar_focused(&self) -> bool {
        match self.screen {
            Screen::PoolConfig => !self.editor_state.focused,
            Screen::Settings => !self.settings_state.focused,
            Screen::PoolStatus => !self.miners_focused,
            Screen::Home | Screen::Bchn | Screen::FileExplorer => true,
        }
    }

    /// The keys the status bar lists.
    pub fn hints(&self) -> Vec<Hint> {
        let mut hints = Vec::with_capacity(5);
        match self.screen {
            Screen::FileExplorer => hints.extend([
                Hint::new("Up/Down", "Navigate"),
                Hint::new("Enter", "Select"),
                Hint::new("Backspace", "Parent folder"),
                Hint::new("Esc", "Cancel"),
            ]),
            Screen::PoolConfig if self.editor_state.input.is_some() => {
                hints.extend([Hint::new("Enter", "Apply"), Hint::new("Esc", "Cancel")])
            }
            Screen::PoolConfig if self.editor_state.focused => hints.extend([
                Hint::new("Up/Down", "Navigate"),
                Hint::new("Enter", "Edit"),
                Hint::new("s", "Save"),
                Hint::new("Esc", "Back"),
            ]),
            Screen::PoolConfig => hints.extend([
                Hint::new("Up/Down", "Sidebar"),
                Hint::new(
                    "Enter",
                    if self.editor.is_some() {
                        "Edit config"
                    } else {
                        "Pick config.toml"
                    },
                ),
                Hint::new("q", "Quit"),
            ]),
            Screen::Settings if self.settings_state.focused => hints.extend([
                Hint::new("Up/Down", "Navigate"),
                Hint::new("Enter", "Browse"),
                Hint::new("Backspace", "Clear"),
                Hint::new("Esc", "Back"),
            ]),
            Screen::Settings => hints.extend([
                Hint::new("Up/Down", "Sidebar"),
                Hint::new("Enter", "Focus settings"),
                Hint::new("q", "Quit"),
            ]),
            Screen::PoolStatus if self.miners_focused => hints.extend([
                Hint::new("Up/Down", "Pick a miner"),
                Hint::new("Enter", "Show its window"),
                Hint::new("Esc", "Back"),
            ]),
            Screen::PoolStatus | Screen::Bchn => {
                hints.extend([
                    Hint::new("Up/Down", "Sidebar"),
                    Hint::new("Left/Right", "Tab"),
                ]);
                if self.screen == Screen::PoolStatus && self.pool_tab == 2 {
                    hints.push(Hint::new("Enter", "Miners"));
                }
                hints.extend([Hint::new("r", "Refresh"), Hint::new("q", "Quit")]);
            }
            Screen::Home => hints.extend([
                Hint::new("Up/Down", "Sidebar"),
                Hint::new("r", "Refresh"),
                Hint::new("q", "Quit"),
            ]),
        }
        hints
    }

    /// Draws the frame.
    pub fn draw(&self, frame: &mut Frame) {
        let now = unix_now();
        let hints = self.hints();
        let notice = match self.screen {
            Screen::Settings => self.settings_state.error.as_deref(),
            _ => None,
        };
        let area = render_shell(
            frame,
            self.sidebar_index,
            self.sidebar_focused(),
            &hints,
            notice,
        );
        let node_config = self
            .node_config_path
            .as_deref()
            .map(|path| path.display().to_string());
        let api_url = self.api.as_ref().map(|api| display_url(api.base()));
        let bchn_url = self
            .connection
            .as_ref()
            .map(|connection| display_url(&connection.bchn_url));
        let expected = self
            .connection
            .as_ref()
            .map(|connection| connection.network);
        match self.screen {
            Screen::Home => home::render(
                frame,
                area,
                &HomeScreen {
                    node_config: node_config.as_deref(),
                    config_error: self.connection_error.as_deref(),
                    network: expected,
                    api_url: api_url.as_deref(),
                    bchn_url: bchn_url.as_deref(),
                    site: &self.site,
                    bchn: &self.bchn_status,
                },
            ),
            Screen::Bchn => bchn::render(
                frame,
                area,
                &bchn::BchnScreen {
                    status: &self.bchn_status,
                    expected,
                    tab: self.bchn_tab,
                    now,
                },
            ),
            Screen::PoolStatus => pool::render(
                frame,
                area,
                &PoolScreen {
                    tab: self.pool_tab,
                    expected,
                    api_url: api_url.as_deref(),
                    now,
                    site: &self.site,
                    chain_info: &self.chain_info,
                    hashrate: &self.hashrate,
                    shares: &self.shares,
                    live_shares: &self.live_shares,
                    miners: &self.miners,
                    miners_focused: self.miners_focused,
                    miners_selected: self.miners_selected,
                    miner: &self.miner,
                    found_blocks: &self.found_blocks,
                    peers: &self.peers,
                    live_peer_events: &self.live_peer_events,
                    live_error: self.live_error.as_deref(),
                },
            ),
            Screen::PoolConfig => config::render(
                frame,
                area,
                &ConfigScreen {
                    path: self.editor.as_ref().and(node_config.as_deref()),
                    rows: &self.config_rows,
                    selected: self.editor_state.selected,
                    focused: self.editor_state.focused,
                    input: self.editor_state.input.as_deref(),
                    warning: self.editor_state.warning.as_deref(),
                    info: self.editor_state.info.as_deref(),
                    dirty: self.editor.as_ref().is_some_and(|editor| editor.dirty),
                },
            ),
            Screen::Settings => {
                let default_dir = self.default_settings_dir.display().to_string();
                let override_dir = self
                    .settings
                    .settings_dir_override
                    .as_deref()
                    .map(|dir| dir.display().to_string());
                let configured = self
                    .settings
                    .node_config_path
                    .as_deref()
                    .map(|path| path.display().to_string());
                settings_view::render(
                    frame,
                    area,
                    &SettingsScreen {
                        node_config: configured.as_deref(),
                        settings_dir_override: override_dir.as_deref(),
                        default_settings_dir: &default_dir,
                        selected: self.settings_state.selected,
                        focused: self.settings_state.focused,
                    },
                );
            }
            Screen::FileExplorer => explorer::render(frame, area, &self.explorer),
        }
    }
}

/// Appends `item`, dropping the oldest once there are more than `limit`.
fn push_capped<T>(items: &mut VecDeque<T>, item: T, limit: usize) {
    items.push_back(item);
    while items.len() > limit {
        items.pop_front();
    }
}

/// Unix seconds now. A clock before 1970 reads as 0, which only makes the
/// ages shown wrong.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_editor::field_index;
    use crate::test_utils::{
        BCH_CHIPNET_CONFIG, sample_bchn_status, sample_miners, sample_peer, sample_share,
        sample_site, screen_text, write_config,
    };

    #[tokio::test]
    async fn test_a_new_app_has_nothing_to_fetch_without_a_config() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        app.tick(Instant::now());
        assert_eq!(app.bchn_status, Remote::Unconfigured);
        assert_eq!(app.site, Remote::Unconfigured);
    }

    #[tokio::test]
    async fn test_opening_a_config_points_at_its_api_and_bchn() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), BCH_CHIPNET_CONFIG);
        let mut app = App::new(Settings::default(), dir.path().into(), Handle::current());
        assert!(app.open_node_config(path));
        assert_eq!(app.connection_error, None);
        assert_eq!(
            app.api.as_ref().map(ApiClient::base),
            Some("http://127.0.0.1:46884")
        );
        assert!(!app.config_rows.is_empty());
    }

    #[tokio::test]
    async fn test_tick_marks_the_home_sources_loading_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), BCH_CHIPNET_CONFIG);
        let mut app = App::new(Settings::default(), dir.path().into(), Handle::current());
        app.open_node_config(path);
        let now = Instant::now();
        app.tick(now);
        assert_eq!(app.site, Remote::Loading);
        assert_eq!(app.bchn_status, Remote::Loading);
        assert!(app.fetches[Source::Network as usize].in_flight);
        assert_eq!(
            app.fetches[Source::Network as usize].last_started,
            Some(now)
        );
        app.tick(now + REFRESH_INTERVAL);
        assert_eq!(
            app.fetches[Source::Network as usize].last_started,
            Some(now)
        );
    }

    #[tokio::test]
    async fn test_an_answer_lands_and_allows_the_next_fetch() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        app.fetches[Source::Network as usize].in_flight = true;
        app.apply(Update {
            generation: app.generation,
            payload: Payload::Network(Ok(sample_site())),
        });
        assert_eq!(app.site, Remote::Ready(sample_site()));
        assert!(!app.fetches[Source::Network as usize].in_flight);
    }

    #[tokio::test]
    async fn test_an_answer_of_an_earlier_config_is_dropped() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        let old_generation = app.generation;
        app.clear_node_config();
        app.apply(Update {
            generation: old_generation,
            payload: Payload::Bchn(Ok(sample_bchn_status())),
        });
        assert_eq!(app.bchn_status, Remote::Unconfigured);
    }

    #[tokio::test]
    async fn test_a_failure_replaces_the_value() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        app.site = Remote::Ready(sample_site());
        app.apply(Update {
            generation: app.generation,
            payload: Payload::Network(Err("boom".into())),
        });
        assert_eq!(app.site, Remote::Failed("boom".into()));
    }

    #[tokio::test]
    async fn test_live_shares_are_capped() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        let now = Instant::now();
        let shares: Vec<Share> = (0..(MAX_LIVE_SHARES as u32 + 10))
            .map(sample_share)
            .collect();
        shares.into_iter().for_each(|share| {
            app.apply_live(LiveMessage::Event(LiveEvent::Share(share)), now);
        });
        assert_eq!(app.live_shares.len(), MAX_LIVE_SHARES);
        assert_eq!(app.live_shares.front().unwrap().height, 10);
    }

    #[tokio::test]
    async fn test_a_peer_event_updates_the_peers_list() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        let now = Instant::now();
        app.peers = Remote::Ready(vec![sample_peer()]);
        app.apply_live(
            LiveMessage::Event(LiveEvent::Peer(PeerEvent {
                peer_id: "12D3new".into(),
                status: PeerStatus::Connected,
            })),
            now,
        );
        app.apply_live(
            LiveMessage::Event(LiveEvent::Peer(PeerEvent {
                peer_id: sample_peer().peer_id,
                status: PeerStatus::Disconnected,
            })),
            now,
        );
        let peers = app.peers.ready().unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].peer_id, "12D3new");
        assert_eq!(app.live_peer_events.len(), 2);
    }

    #[tokio::test]
    async fn test_a_peer_event_leaves_a_failed_peers_list_alone() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        app.peers = Remote::Failed("Unauthorized".into());
        app.apply_live(
            LiveMessage::Event(LiveEvent::Peer(PeerEvent {
                peer_id: "12D3new".into(),
                status: PeerStatus::Connected,
            })),
            Instant::now(),
        );
        assert_eq!(app.peers, Remote::Failed("Unauthorized".into()));
    }

    #[tokio::test]
    async fn test_an_ended_feed_waits_before_retrying() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        let now = Instant::now();
        app.apply_live(LiveMessage::Ended("closed".into()), now);
        assert_eq!(app.live_error.as_deref(), Some("closed"));
        assert_eq!(app.live_retry_at, Some(now + LIVE_RETRY_INTERVAL));
    }

    #[tokio::test]
    async fn test_a_feed_gone_without_a_word_is_ended_and_retried() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        let (sender, receiver) = mpsc::channel(1);
        drop(sender);
        app.live_receiver = Some(receiver);
        let now = Instant::now();
        app.drain(now);
        assert_eq!(app.live_error.as_deref(), Some("the live feed stopped"));
        assert!(app.live_receiver.is_none());
        assert_eq!(app.live_retry_at, Some(now + LIVE_RETRY_INTERVAL));
    }

    #[tokio::test]
    async fn test_picking_a_miner_asks_for_its_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), BCH_CHIPNET_CONFIG);
        let mut app = App::new(Settings::default(), dir.path().into(), Handle::current());
        app.open_node_config(path);
        app.select_sidebar(3);
        app.pool_tab = 2;
        app.miners = Remote::Ready(sample_miners());
        app.pick_miner(Instant::now());
        assert_eq!(
            app.miner_address.as_deref(),
            Some(sample_miners().miners[0].address.as_str())
        );
        assert_eq!(app.miner, Remote::Loading);
        assert!(app.wanted_sources().contains(&Source::Miner));
    }

    #[tokio::test]
    async fn test_a_shorter_miners_page_moves_the_cursor_onto_it() {
        let mut app = App::new(Settings::default(), PathBuf::from("."), Handle::current());
        app.miners_selected = 7;
        app.apply(Update {
            generation: app.generation,
            payload: Payload::Miners(Ok(sample_miners())),
        });
        assert_eq!(app.miners_selected, 0);
    }

    #[tokio::test]
    async fn test_commit_edit_reports_a_config_the_node_would_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), BCH_CHIPNET_CONFIG);
        let mut app = App::new(Settings::default(), dir.path().into(), Handle::current());
        app.open_node_config(path);
        app.editor_state.selected = 0;
        app.editor_state.input = Some("mainnet".into());
        app.commit_edit();
        assert!(app.editor_state.warning.is_some());
        assert_eq!(app.config_rows[0].value.as_deref(), Some("mainnet"));
    }

    #[tokio::test]
    async fn test_saving_a_picked_config_remembers_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), BCH_CHIPNET_CONFIG);
        let settings_dir = dir.path().join("settings");
        let mut app = App::new(Settings::default(), settings_dir.clone(), Handle::current());
        app.explorer_trigger = Some(ExplorerTrigger::PoolConfig);
        app.file_picked(path.clone());
        assert_eq!(app.settings.node_config_path, Some(path.clone()));
        assert_eq!(
            settings::load(&settings_dir).unwrap().node_config_path,
            Some(path)
        );
    }

    #[tokio::test]
    async fn test_draw_never_shows_the_rpc_password() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), BCH_CHIPNET_CONFIG);
        let mut app = App::new(Settings::default(), dir.path().into(), Handle::current());
        app.open_node_config(path);
        app.select_sidebar(2);
        app.editor_state.focused = true;
        app.editor_state.selected = field_index("bitcoinrpc", "password").unwrap();
        let text = screen_text(160, 40, |frame| app.draw(frame));
        assert!(!text.contains("chipnet-rpc-secret"), "{text}");
        assert!(text.contains("********"), "{text}");
    }
}
