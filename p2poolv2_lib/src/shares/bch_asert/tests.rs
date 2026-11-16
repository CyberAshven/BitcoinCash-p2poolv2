use super::*;
use bitcoin::block::Header;
use bitcoin::consensus::deserialize;

/// ASERT after a parent placed by its height through the network's pins,
/// as these tests read real headers whose parents are not known headers.
pub(crate) trait ByHeight {
    fn next_target_at(&self, parent_height: u32, parent_time: u32) -> Option<U256>;
    fn next_bits_at(&self, parent_height: u32, parent_time: u32) -> Option<CompactTarget>;
    fn min_difficulty_gap_at(&self, parent_height: u32) -> u32;
}

impl ByHeight for BchAsert {
    fn next_target_at(&self, parent_height: u32, parent_time: u32) -> Option<U256> {
        self.next_target(self.tick_schedule.tick_info(parent_height), parent_time)
    }

    fn next_bits_at(&self, parent_height: u32, parent_time: u32) -> Option<CompactTarget> {
        self.next_bits(self.tick_schedule.tick_info(parent_height), parent_time)
    }

    fn min_difficulty_gap_at(&self, parent_height: u32) -> u32 {
        self.tick_schedule
            .tick_info(parent_height)
            .min_difficulty_gap()
    }
}

/// The aserti3-2d specification's vector runs, from BCHN's QA assets
/// (`aserti3-2d/README.md` beside this file).
const SPECIFICATION_RUNS: [(&str, &str); 12] = [
    ("run01", include_str!("aserti3-2d/run01")),
    ("run02", include_str!("aserti3-2d/run02")),
    ("run03", include_str!("aserti3-2d/run03")),
    ("run04", include_str!("aserti3-2d/run04")),
    ("run05", include_str!("aserti3-2d/run05")),
    ("run06", include_str!("aserti3-2d/run06")),
    ("run07", include_str!("aserti3-2d/run07")),
    ("run08", include_str!("aserti3-2d/run08")),
    ("run09", include_str!("aserti3-2d/run09")),
    ("run10", include_str!("aserti3-2d/run10")),
    ("run11", include_str!("aserti3-2d/run11")),
    ("run12", include_str!("aserti3-2d/run12")),
];

/// The specification's half-life: two days.
const SPECIFICATION_HALF_LIFE: i64 = 2 * 24 * 60 * 60;

fn run_parameter(run: &str, name: &str) -> String {
    let prefix = format!("##   {name}: ");
    run.lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
        .unwrap_or_else(|| panic!("run has no {name}"))
        .trim()
        .to_string()
}

fn parse_bits(text: &str) -> u32 {
    u32::from_str_radix(text.trim_start_matches("0x"), 16).unwrap()
}

/// Every line of every run: the bits BCHN's algorithm gives the block after
/// (height, time) match the specification's, from the run's anchor.
#[test]
fn every_specification_vector_matches() {
    let mut checked = 0;
    for (name, run) in SPECIFICATION_RUNS {
        let anchor_height: i128 = run_parameter(run, "anchor height").parse().unwrap();
        let anchor_parent_time: i64 = run_parameter(run, "anchor parent time").parse().unwrap();
        let anchor_bits = parse_bits(&run_parameter(run, "anchor nBits"));
        let iterations: usize = run_parameter(run, "iterations").parse().unwrap();
        let reference = set_compact(anchor_bits).unwrap();
        let mut lines = 0;
        for line in run
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let height: i128 = fields[1].parse().unwrap();
            let time: i64 = fields[2].parse().unwrap();
            let expected = parse_bits(fields[3]);
            let target = calculate_asert(
                reference,
                time - anchor_parent_time,
                i64::try_from(height - anchor_height).unwrap() * BCH_BLOCK_SPACING,
                BCH_BLOCK_SPACING,
                BCH_BLOCK_SPACING,
                SPECIFICATION_HALF_LIFE,
                bch_pow_limit(),
            )
            .unwrap_or_else(|| panic!("{name}: {line} has no target"));
            assert_eq!(
                get_compact(target).to_consensus(),
                expected,
                "{name}: {line}"
            );
            lines += 1;
        }
        assert_eq!(lines, iterations, "{name}");
        checked += lines;
    }
    assert_eq!(checked, 14_000);
}

/// Consecutive Bitcoin Cash mainnet headers, `(parent height, parent,
/// child)`, as a Fulcrum server (`bch.imaginary.cash`) returned them on
/// 2026-10-23: the first two ASERT blocks after anchor 661,647, then pairs
/// every 50,000 blocks and one near the tip.
pub(crate) const MAINNET_PAIRS: [(u32, &str, &str); 9] = [
    (
        661647,
        "000000202df7a2e0562ebbbd8dc95ca6669c4f1ba888484c2cc7e403000000000000000067418c23d8901e49555725fb2e37adfb9ed29a05833eb4774d53b63b44ba457e8937b15ffeda04183c2b1978",
        "000000202fe6ee2db04b6575ad185521133598e3590d787a4bed8300000000000000000042faaa7bb98abdfb6f780417b1c0eb7873cd49e048feef273d5e270e1eb223855938b15fd0e0041888420330",
    ),
    (
        661648,
        "000000202fe6ee2db04b6575ad185521133598e3590d787a4bed8300000000000000000042faaa7bb98abdfb6f780417b1c0eb7873cd49e048feef273d5e270e1eb223855938b15fd0e0041888420330",
        "0000c020cea8b5f909054a0befc47110914cb7b8248d81411c479e020000000000000000039ea56fe7dfc4d384002372286a5c024853f44598a7ab6263c601744a51921a913cb15fdcde04186567266f",
    ),
    (
        700000,
        "002000202c59ce4f2b8b307624e76e10c7aebd09963a2680b2ebf1030000000000000000fa725fff16fcb89a6c4e204e9a0cacb7c2a57d05b2426558b0002c3e5ee9af1979271161ae060618cbc1b42e",
        "00000020774e6730410f63e2a215c09aa8ed024915fd6374ba08d903000000000000000011068e9235b44bb48bf965d222cfb7cf69c89e07e9427d9d4cd5c76585407908992f11611e07061814283d9f",
    ),
    (
        750000,
        "000020200d973e9438910cb26625473e0e7d554857c7bb9ff675dc00000000000000000052e8e56b1d4cc3c669e08efaae4a5b8d461274090c30bca57f349ff5d41eab86769fdb62d64207183836351f",
        "00002020ba80c94e1ba55702cb565ee84674f7bc3025b068f6b821060000000000000000bb6a0cfc2ed0bf35dda3cc9f9a25c75ddb7909c8e752410fb267e4d385256843c6a0db62474107186a64e2c4",
    ),
    (
        800000,
        "0000602093ea70b294aeef69db85981bd387285824fc5af732e487010000000000000000135576e1ba9cae5509d60b4fbb9e66612f35b5ee8db264cb12612d46cc8db60b5e34a164d46b021864abbb1d",
        "00000020115363e55b7097e57b2b83bf9b9ddadd33a06a4baf2a010100000000000000006ea092a1652e9b1675c57a76379b6600309dcbb31e37e22a9993095a60f06316a634a164aa6a02187cd43c29",
    ),
    (
        850000,
        "0040b52957059c0f6ef48ea083c54ac1062b90c8e2cec01a49afc5000000000000000000a60263111412a3bff43feb38b0465cec4fb84798a12339bfac4d12a6b7d79903d1976a661c3102188ed371b9",
        "00201a2c740936a7ef9e65dc3b367028b7d431332452865faeefd30000000000000000006095d47320b19ca1b312f829fd7c66debf5e2104a41265033513966123a5a44aad996a669d30021843e30391",
    ),
    (
        900000,
        "0000002c6a44d1cc30a114e8cc24929bd93959c0c3d92ad0ebcf1501000000000000000084e964b962e4fd15ab8992bab3258c78e4a2a47b4fe9ae32d07f1e1297ded1dbd8c03468446f0218189b8ea3",
        "00000020cfe15ffb6a1d5cd58bfeceb301e9027dd69f03c54594ff00000000000000000087b779f4afc6b38a85192d2e66acb95bce97e6c4c4210b1f56e3949e7887e41599c23468266e0218cfd2e925",
    ),
    (
        950000,
        "00c0942d13697551f559e615585220d4ee786e7a72254117dfec54010000000000000000366b14cfaacb3510c7bae75dcc530dbe0f322188e5c78eb8551d917769340aeb9195fc69387601186089fb42",
        "00600020bffa3885a75e9c343b8f21f99cc5e38efd21cde45b6546000000000000000000832c8cd11371c463747c468710bb2d120d3355b17a89d0a8f92c0b1bacb86fbb8b9bfc695176011890b46c0b",
    ),
    (
        974000,
        "00a07b208e797095009824cbf43ddb69ee295ed3a1312ac807459901000000000000000077eccd4944b4830f9930f2cbc8024affdf0dc92001779e882b962facf466fb2b80d9d96a2d3002186ada69be",
        "0040aa2249c7a2ccff1dcc7df964811c3fc65fe0c9f2bcd313329c0000000000000000002e79ec0f05c0d315c5b36f9710aaad19a572308f0c9d416c7aead976d3a6f02f83dad96a013102180551d52c",
    ),
];

/// Consecutive chipnet headers, as `chipnet.imaginary.cash` returned them on
/// 2026-10-23: the first two ASERT blocks after anchor 16,844, then later
/// pairs, including minimum-difficulty block 300,001 (more than 20 minutes
/// after its parent) and the ASERT block on top of it.
pub(crate) const CHIPNET_PAIRS: [(u32, &str, &str); 9] = [
    (
        16844,
        "000000205581230d485df30b62e4b569b58df654c72e958af76893ad6278dc8b000000000e50f6458dcdbc3f3d677ba6e7acd7922b6a92562462e8d3faa2169a6f28ef7ed144b15fffff001d08f7a0eb",
        "00000020fcf0daa23858e697da84c7dcac8a4f234c405df7d3c1662bee70256000000000fbb448bc17b4d87d13d9aad7510186573e395efd8b57f1576c74bb7649910ec52b46b15fffff001d0a1e99f8",
    ),
    (
        16845,
        "00000020fcf0daa23858e697da84c7dcac8a4f234c405df7d3c1662bee70256000000000fbb448bc17b4d87d13d9aad7510186573e395efd8b57f1576c74bb7649910ec52b46b15fffff001d0a1e99f8",
        "00000020773074bd864d47a4db82abbb689a69088a705f6ac980fe348f5b32fb000000007eb546d8bd7ae082baa147df2e9760e6612e2d08e389f2148d112240cf5521481f4ab15fffff001d0554d69f",
    ),
    (
        100000,
        "00000020be4b33aab5eeb12c9e40a745d76f43bb2d001d644a84075765a0030000000000b47e7986e612e7f6ac6416b7ad8081749ec254b24dc2c027a34c4e3af3a7f2e68712aa62cc20731bceda8b6e",
        "000000209c953712565950a804ea409315e958745c1bb9d076f2dc04c9e410000000000047759d8010135fc512f9dcc4208a01442e03affb8b809d77fa9ed71e5517be24ad12aa628c31731b450092f7",
    ),
    (
        200000,
        "00000020270088a5d78fd25a32c8bf3f0704aacbd77211bfee9bd5e9afb3dd630000000092c12e2a7f7fc79c64a79ac03e51fa26f819c13906bd1a5612cd6dd0cd568972e6083e66cc9d731c85cf1b8d",
        "00000020057c35c99f26ac74547bc2f4c7bd19d60a071962fce3402a46997d5d000000001f4e4bf48f1d0c6da2e98ef663e255e84a6a7c817fb19e02f1c0406a43191aae520a3e6693f46c1c11e6c5b8",
    ),
    (
        299999,
        "000000202624b6fce1cfb175992b4f635443a23c4fba0b7d4047dd5aeb54571f00000000ea59a5fe12ba55770f085bb8fa67a76901c08f7e4c3cc2db85ad811a960ed6a9bf7ad16952b92d1c058f6993",
        "000000202cbcbd2e79a725ad3656f0598c6b3e77ad265ea215baf3bee8f1200200000000f4ce8156329d29f7bd3f2dddd3158408db01da18a84aa30ddf7df79367cfe5a93a7dd169941b2b1c02d0b3f6",
    ),
    (
        300000,
        "000000202cbcbd2e79a725ad3656f0598c6b3e77ad265ea215baf3bee8f1200200000000f4ce8156329d29f7bd3f2dddd3158408db01da18a84aa30ddf7df79367cfe5a93a7dd169941b2b1c02d0b3f6",
        "000000206ad0f579154eda0fe83fd4cf46d4b9be7716d64c15baa49fe3be2f14000000007e74fb11a335ce614c53dd3f841ed7c0b9f044ac11851c5b85e06525faa2a2999483d169ffff001d0836e56b",
    ),
    (
        300001,
        "000000206ad0f579154eda0fe83fd4cf46d4b9be7716d64c15baa49fe3be2f14000000007e74fb11a335ce614c53dd3f841ed7c0b9f044ac11851c5b85e06525faa2a2999483d169ffff001d0836e56b",
        "00000020c5d7d2b0ceec4b1abd7e0959658bf74bce9d1612964b22006ff8155700000000d60d4fce09b3478d9d674fb1066d99c43f785fe7227145d792f64c6300e9c729f383d1698be2341c38f1a94a",
    ),
    (
        328000,
        "00e0ff3f29e6c8f5f995ec4d99ba27eb63c8e650738e925ef01fa5b6ed0800000000000042642408c6d9a42898abe6a594824f77fea53bc9406aeed8eb7b95eedaa6d67e14e5d06acb01141ae1242807",
        "000000204e7db40f063cc1ddb05e0cb7ba2e469deaeb8824ea71fa6c711300000000000027df2adc2bb71594bf17ed39dd4b9448b2e52b360c421c5a8cf85b28829d18e650e9d06aac94131a7176eda5",
    ),
    (
        329000,
        "e00000202a9383e8970c1414ba5bd6547840ebe60fb4d8d4b4c301a1ca34000000000000303b9396757836e6842e9b46824581a633c7ebb23de0a45362b288b5b30e37d6731fda6a6d97321abd42032d",
        "e0ffff3f593afa32a74a48eb223471d047bab06ec694e96916e49d466d02000000000000180a3af09db7f4e6e56e5cc3d9744d3b5608367eb54d90a67917bc084de9e23d7d20da6aee06311adac55d98",
    ),
];

pub(crate) fn header(hex_header: &str) -> Header {
    deserialize(&hex::decode(hex_header).unwrap()).unwrap()
}

/// Each child links to its parent and meets its own target, so these are
/// real, consecutive, mined headers.
fn real_pair(parent_hex: &str, child_hex: &str) -> (Header, Header) {
    let (parent, child) = (header(parent_hex), header(child_hex));
    assert_eq!(child.prev_blockhash, parent.block_hash());
    assert!(child.target().is_met_by(child.block_hash()));
    (parent, child)
}

#[test]
fn real_mainnet_headers_carry_the_bits_asert_gives_them() {
    for (parent_height, parent_hex, child_hex) in MAINNET_PAIRS {
        let (parent, child) = real_pair(parent_hex, child_hex);
        assert_eq!(
            BchAsert::MAINNET.next_bits_at(parent_height, parent.time),
            Some(child.bits),
            "block {}",
            parent_height + 1
        );
        // The header check, which has only the child, accepts it.
        BchAsert::MAINNET
            .check_header_bits(child.bits, child.time, u64::from(parent_height) + 1, None)
            .unwrap();
    }
}

#[test]
fn real_chipnet_headers_carry_the_bits_asert_gives_them() {
    let mut minimum_difficulty_blocks = 0;
    for (parent_height, parent_hex, child_hex) in CHIPNET_PAIRS {
        let (parent, child) = real_pair(parent_hex, child_hex);
        // BCHN's test-network rule: a block more than 20 minutes after its
        // parent may use the proof-of-work limit.
        let expected = if child.time > parent.time + 2 * BCH_BLOCK_SPACING as u32 {
            minimum_difficulty_blocks += 1;
            CompactTarget::from_consensus(BCH_POW_LIMIT_BITS)
        } else {
            BchAsert::CHIPNET
                .next_bits_at(parent_height, parent.time)
                .unwrap()
        };
        assert_eq!(child.bits, expected, "block {}", parent_height + 1);
        BchAsert::CHIPNET
            .check_header_bits(child.bits, child.time, u64::from(parent_height) + 1, None)
            .unwrap();
    }
    assert_eq!(minimum_difficulty_blocks, 1);
}

/// On schedule from the anchor the target is the anchor's own.
#[test]
fn on_schedule_after_the_anchor_keeps_the_anchor_target() {
    let mainnet = BchAsert::MAINNET;
    assert_eq!(
        mainnet
            .next_bits_at(
                mainnet.anchor_height,
                (mainnet.anchor_parent_time + BCH_BLOCK_SPACING) as u32
            )
            .unwrap()
            .to_consensus(),
        mainnet.anchor_bits
    );
    assert_eq!(
        mainnet.next_bits_at(mainnet.anchor_height - 1, u32::MAX),
        None
    );
    // Far behind schedule it stays at the limit; far ahead it never reaches 0.
    let chipnet = BchAsert::CHIPNET;
    assert_eq!(
        chipnet
            .next_bits_at(chipnet.anchor_height + 1, u32::MAX)
            .unwrap()
            .to_consensus(),
        BCH_POW_LIMIT_BITS
    );
    assert!(!chipnet.next_target_at(u32::MAX, 0).unwrap().is_zero());
}

/// Mainnet one block past the anchor, on schedule: the computed target is
/// the anchor's, `0x04dafe << 168`. Its half is exactly `0x18026d7f` and
/// 1.05 times it lies between `0x18051924` and `0x18051925`, so the band's
/// edges are exact bits.
#[test]
fn mainnet_band_is_half_to_one_point_zero_five() {
    let mainnet = BchAsert::MAINNET;
    let height = u64::from(mainnet.anchor_height) + 1;
    let time = (mainnet.anchor_parent_time + BCH_BLOCK_SPACING) as u32;
    let check = |bits: u32| {
        mainnet.check_header_bits(CompactTarget::from_consensus(bits), time, height, None)
    };
    check(0x1804_dafe).unwrap();
    check(0x1802_6d7f).unwrap();
    assert!(check(0x1802_6d7e).unwrap_err().contains("harder"));
    check(0x1805_1924).unwrap();
    assert!(check(0x1805_1925).unwrap_err().contains("easier"));
    // Mainnet allows no minimum-difficulty block.
    assert!(check(BCH_POW_LIMIT_BITS).is_err());
}

/// A token share's nBits must pass the band by the pins and by its known
/// parent's own values, as peers check it at the header gate and in block
/// validation. Mainnet after its switch, not pinned: a header with the nBits
/// its one-minute parent requires passes the parent's band, but the pins
/// count ten-minute blocks and want a target over ten times harder, so peers'
/// gates would refuse it and it is no token share worth handing out.
#[test]
fn a_token_header_must_pass_both_bands_peers_check() {
    let mainnet = BchAsert::MAINNET;
    let height: u32 = 1_005_400;
    // A switch after block 1,003,000.
    let parent = TickInfo {
        ticks: 600 * 1_003_000 + 60 * i64::from(height - 1 - 1_003_000),
        next_interval: 60,
    };
    let parent_time =
        (mainnet.anchor_parent_time + parent.ticks - 600 * i64::from(mainnet.anchor_height)) as u32;
    let bits = mainnet.next_bits(parent, parent_time).unwrap();
    let time = parent_time + 60;
    let height = u64::from(height);

    mainnet
        .check_header_bits(bits, time, height, Some(parent))
        .unwrap();
    assert!(
        mainnet
            .check_header_bits(bits, time, height, None)
            .unwrap_err()
            .contains("easier")
    );
    assert!(
        mainnet
            .check_header_bits_for_peers(bits, time, height, Some(parent))
            .is_err()
    );
    assert!(
        mainnet
            .check_header_bits_for_peers(bits, time, height, None)
            .is_err()
    );
    // Where the pins agree with the parent, as before any switch, both pass.
    let ten_minute = mainnet.tick_schedule.tick_info(661_700);
    let before = (mainnet.anchor_parent_time + ten_minute.ticks
        - 600 * i64::from(mainnet.anchor_height)) as u32;
    let bits = mainnet.next_bits(ten_minute, before).unwrap();
    mainnet
        .check_header_bits_for_peers(bits, before + 600, 661_701, Some(ten_minute))
        .unwrap();
}

/// Chipnet: up to eight times harder, up to eight times easier, or the
/// limit's bits, as the SAFA covenant's chipnet band.
#[test]
fn test_network_band_is_an_eighth_to_eight_times_or_the_limit() {
    let chipnet = BchAsert::CHIPNET;
    // Twenty half-lives ahead of schedule: the target is about 2^-20 of the
    // limit, far from both clamps.
    let height = u64::from(chipnet.anchor_height) + 1 + 20 * 6;
    let time = (chipnet.anchor_parent_time + BCH_BLOCK_SPACING) as u32;
    let computed = chipnet.next_target_at(height as u32 - 1, time).unwrap();
    let check = |target: U256| chipnet.check_header_bits(get_compact(target), time, height, None);
    check(computed).unwrap();
    let eighth = computed / U256::from(8);
    check(eighth).unwrap();
    assert!(
        check(computed / U256::from(16))
            .unwrap_err()
            .contains("harder")
    );
    check(computed * U256::from(2)).unwrap();
    check(computed * U256::from(8)).unwrap();
    assert!(
        check(computed * U256::from(16))
            .unwrap_err()
            .contains("easier")
    );
    chipnet
        .check_header_bits(
            CompactTarget::from_consensus(BCH_POW_LIMIT_BITS),
            time,
            height,
            None,
        )
        .unwrap();
}

/// The exact `computed * 8` edge, where ASERT clamps to 1 so the edge and
/// one above it are both exact compact values (the SAFA tool tests
/// the same two bits).
#[test]
fn test_network_easy_edge_is_exactly_eight_times() {
    let chipnet = BchAsert::CHIPNET;
    let height = u64::from(chipnet.anchor_height) + 10_001;
    let time = (chipnet.anchor_parent_time + BCH_BLOCK_SPACING) as u32;
    assert_eq!(
        chipnet.next_target_at(height as u32 - 1, time),
        Some(U256::one())
    );
    assert_eq!(set_compact(0x0108_0000), Some(U256::from(8)));
    chipnet
        .check_header_bits(
            CompactTarget::from_consensus(0x0108_0000),
            time,
            height,
            None,
        )
        .unwrap();
    assert_eq!(set_compact(0x0109_0000), Some(U256::from(9)));
    assert!(
        chipnet
            .check_header_bits(
                CompactTarget::from_consensus(0x0109_0000),
                time,
                height,
                None
            )
            .unwrap_err()
            .contains("easier")
    );
}

/// Real chipnet block 329,001 with its own nBits and a time two hours
/// before its real parent's, which BCHN accepts when the parent is dated
/// ahead. Only the time moves, so this checks the band, not proof of work.
/// The old `computed * 1.05` easy side refused it from about 253 seconds.
#[test]
fn real_chipnet_header_two_hours_before_its_parent_passes() {
    let (parent_height, parent_hex, child_hex) = CHIPNET_PAIRS[CHIPNET_PAIRS.len() - 1];
    let (parent, child) = real_pair(parent_hex, child_hex);
    let height = u64::from(parent_height) + 1;
    assert_eq!(height, 329_001);
    assert_eq!(child.bits.to_consensus(), 0x1a31_06ee);
    assert_eq!(
        BchAsert::CHIPNET.next_bits_at(parent_height, parent.time),
        Some(child.bits)
    );
    BchAsert::CHIPNET
        .check_header_bits(child.bits, parent.time - 200, height, None)
        .unwrap();
    BchAsert::CHIPNET
        .check_header_bits(child.bits, parent.time - 260, height, None)
        .unwrap();
    BchAsert::CHIPNET
        .check_header_bits(child.bits, parent.time - 600, height, None)
        .unwrap();
    BchAsert::CHIPNET
        .check_header_bits(child.bits, parent.time - 3_600, height, None)
        .unwrap();
    BchAsert::CHIPNET
        .check_header_bits(child.bits, parent.time - 7_200, height, None)
        .unwrap();
    // The same two hours on mainnet's band would need 2^(1/24), about
    // 1.029, which 1.05 covers.
    let mut old_band = BchAsert::CHIPNET;
    (old_band.easy_numerator, old_band.easy_denominator) = (105, 100);
    old_band
        .check_header_bits(child.bits, parent.time - 200, height, None)
        .unwrap();
    assert!(
        old_band
            .check_header_bits(child.bits, parent.time - 260, height, None)
            .is_err()
    );
}

/// The SAFA covenant's verdict on a header's nBits (`p2poolv2_safa`):
/// Function 7 of the covenant compiled for a network, run in the BCH 2026
/// VM, and the tool's Rust model of it, which must agree.
struct SafaBand {
    parameters: p2poolv2_safa::NetworkParameters,
    library: Vec<u8>,
}

impl SafaBand {
    fn new(parameters: p2poolv2_safa::NetworkParameters) -> Self {
        let library = p2poolv2_safa::compile_library_for(&parameters, "").unwrap();
        Self {
            parameters,
            library,
        }
    }

    fn for_network(network: &str) -> Self {
        Self::new(p2poolv2_safa::network_parameters(network).unwrap())
    }

    fn accepts(&self, bits: CompactTarget, time: u32, height: u32) -> bool {
        let bits = bits.to_consensus();
        let stack = p2poolv2_safa::evaluate_function(
            &self.library,
            7,
            &[
                bits.to_le_bytes().to_vec(),
                time.to_le_bytes().to_vec(),
                libauth::big_int_to_vm_number(&p2poolv2_safa::BigInt::from_u64(u64::from(height))),
            ],
        );
        // A header outside the band leaves false. Invalid nBits and a height
        // at or below the anchor fail the script, as they fail the gate.
        let in_vm = stack.is_ok_and(|stack| stack.last().is_some_and(|top| top.as_slice() == [1]));
        assert_eq!(
            in_vm,
            p2poolv2_safa::asert_band_ok_for(&self.parameters, bits, time, height).unwrap(),
            "{bits:#010x} at height {height}, time {time}"
        );
        in_vm
    }
}

/// nBits around a computed target and both edges of either band: the
/// proof-of-work limit's, and for each multiple the rounded compact value
/// and one step either side of it.
fn band_candidates(computed: U256) -> Vec<CompactTarget> {
    let mut candidates = vec![CompactTarget::from_consensus(BCH_POW_LIMIT_BITS)];
    for (numerator, denominator) in [
        (1, 16),
        (1, 9),
        (1, 8),
        (1, 2),
        (1, 1),
        (104, 100),
        (105, 100),
        (106, 100),
        (2, 1),
        (8, 1),
        (9, 1),
        (16, 1),
    ] {
        let target = computed * U256::from(numerator) / U256::from(denominator);
        if !target.is_zero() && target <= bch_pow_limit() {
            let compact = get_compact(target);
            candidates.push(compact);
            let raw = compact.to_consensus();
            candidates.push(CompactTarget::from_consensus(raw + 1));
            candidates.push(CompactTarget::from_consensus(raw - 1));
        }
    }
    candidates
}

/// The header gate and the SAFA covenant give one verdict on every candidate
/// around `asert`'s target for a header at `height` dated `time`. Returns
/// how many were compared.
fn gate_and_covenant_agree(asert: &BchAsert, safa: &SafaBand, height: u32, time: u32) -> usize {
    let computed = asert.next_target_at(height - 1, time).unwrap();
    let candidates = band_candidates(computed);
    let mut verdicts = [0, 0];
    for bits in &candidates {
        let gate = asert
            .check_header_bits(*bits, time, u64::from(height), None)
            .is_ok();
        assert_eq!(
            gate,
            safa.accepts(*bits, time, height),
            "{:#010x} at height {height}, time {time}",
            bits.to_consensus()
        );
        verdicts[usize::from(gate)] += 1;
    }
    // The candidates straddle the band: some pass and, unless the band
    // reaches both clamps, some fail.
    assert!(verdicts[1] > 0, "height {height}: nothing passes");
    candidates.len()
}

/// The header check and the SAFA covenant's band (`p2poolv2_safa`, compiled
/// and run in the BCH 2026 VM) agree on both networks, around and across
/// both edges, with the proof-of-work limit's bits and with real headers
/// dated from two hours before to two hours after their parents. On chipnet
/// that includes headers after its switch to one-minute blocks, which both
/// count by the pinned block.
#[test]
fn header_band_matches_the_safa_tool() {
    let mut checked = 0;
    // Chipnet's block 332,460 and its time: later blocks a minute apart keep
    // its target.
    let (fast_height, fast_time) = (332_460_u32, 1_794_746_573_u32);
    for (asert, network, pairs) in [
        (BchAsert::MAINNET, "mainnet", &MAINNET_PAIRS[..]),
        (BchAsert::CHIPNET, "chipnet", &CHIPNET_PAIRS[..]),
    ] {
        let safa = SafaBand::for_network(network);
        let mut headers = vec![
            (
                asert.anchor_height + 1,
                (asert.anchor_parent_time + 600) as u32,
            ),
            (
                asert.anchor_height + 121,
                (asert.anchor_parent_time + 600) as u32,
            ),
            (
                asert.anchor_height + 10_001,
                (asert.anchor_parent_time + 600) as u32,
            ),
            (
                asert.anchor_height + 50,
                (asert.anchor_parent_time + 900_000) as u32,
            ),
        ];
        if network == "chipnet" {
            headers.extend([
                // The last ten-minute header, the first with a 60-tick
                // interval and the first on a 60-tick parent.
                (332_450, 1_794_745_996),
                (332_451, 1_794_746_156),
                (332_452, 1_794_746_219),
                (fast_height, fast_time),
                (fast_height + 1_440, fast_time + 1_440 * 60),
                (fast_height + 100_000, fast_time + 100_000 * 60),
                // Two hours early and an hour late on a one-minute parent.
                (fast_height + 1_440, fast_time + 1_440 * 60 - 7_200),
                (fast_height + 1_440, fast_time + 1_440 * 60 + 3_600),
            ]);
        }
        for (height, time) in headers {
            checked += gate_and_covenant_agree(&asert, &safa, height, time);
        }
        for (parent_height, parent_hex, child_hex) in pairs {
            let (parent, child) = real_pair(parent_hex, child_hex);
            for skew in [-7_200_i64, -3_600, -260, -200, 0, 600, 7_200] {
                let time = (i64::from(parent.time) + skew) as u32;
                let height = parent_height + 1;
                assert_eq!(
                    asert
                        .check_header_bits(child.bits, time, u64::from(height), None)
                        .is_ok(),
                    safa.accepts(child.bits, time, height),
                    "{network} block {height} at {skew} s"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 500, "{checked}");
}

/// The SAFA covenant is compiled with the header gate's own numbers: each
/// network's anchor, half-life and band, and the switch the gate pins.
/// Pinning a switch in one and not the other fails here.
#[test]
fn the_safa_covenant_pins_the_switches_the_header_gate_pins() {
    for (asert, network) in [
        (BchAsert::MAINNET, "mainnet"),
        (BchAsert::CHIPNET, "chipnet"),
    ] {
        let safa = p2poolv2_safa::network_parameters(network).unwrap();
        assert_eq!(safa.anchor_height, asert.anchor_height, "{network}");
        assert_eq!(safa.anchor_bits, asert.anchor_bits, "{network}");
        assert_eq!(
            i64::from(safa.anchor_parent_time),
            asert.anchor_parent_time,
            "{network}"
        );
        assert_eq!(i64::from(safa.half_life), asert.half_life, "{network}");
        assert_eq!(safa.allow_pow_limit, asert.allow_pow_limit, "{network}");
        assert_eq!(
            (
                safa.hard_divisor,
                safa.easy_numerator,
                safa.easy_denominator
            ),
            (
                asert.hard_divisor,
                asert.easy_numerator,
                asert.easy_denominator
            ),
            "{network}"
        );
        let pinned: Vec<(u32, i64)> = asert
            .tick_schedule
            .switches()
            .map(|switch| (switch.height, switch.interval))
            .collect();
        let compiled: Vec<(u32, i64)> = safa
            .interval_switch
            .iter()
            .map(|switch| (switch.height, i64::from(switch.interval)))
            .collect();
        assert_eq!(compiled, pinned, "{network}");
        assert_eq!(
            i64::from(p2poolv2_safa::BCH_BLOCK_SPACING),
            BCH_BLOCK_SPACING
        );
    }
}

/// Every real chipnet block around the one-minute activation, dated from
/// two hours before to two hours after its own time, with its own nBits,
/// the proof-of-work limit's and the block before's: the header gate
/// without a known parent and the SAFA covenant give one verdict, and both
/// take each block as it was mined. So do they on heights and nBits either
/// refuses outright.
#[test]
fn the_safa_covenant_band_is_the_header_gates_around_chipnets_activation() {
    let chipnet = BchAsert::CHIPNET;
    let safa = SafaBand::for_network("chipnet");
    let mut checked = 0;
    let mut refused_after_the_switch = 0;
    let mut previous_bits = CHIPNET_AROUND_ACTIVATION[0].2;
    for (height, block_time, block_bits) in CHIPNET_AROUND_ACTIVATION {
        for skew in [-7_200_i64, -3_600, -120, 0, 120, 600, 7_200] {
            let time = (i64::from(block_time) + skew) as u32;
            for bits in [block_bits, BCH_POW_LIMIT_BITS, previous_bits] {
                let bits = CompactTarget::from_consensus(bits);
                let gate = chipnet
                    .check_header_bits(bits, time, u64::from(height), None)
                    .is_ok();
                assert_eq!(
                    gate,
                    safa.accepts(bits, time, height),
                    "block {height} at {skew} s with {:#010x}",
                    bits.to_consensus()
                );
                if skew == 0 && bits.to_consensus() == block_bits {
                    assert!(gate, "block {height} as mined");
                }
                refused_after_the_switch += usize::from(!gate && height > 332_450);
                checked += 1;
            }
        }
        previous_bits = block_bits;
    }
    assert_eq!(checked, 22 * 7 * 3);
    // The comparison is not all passes: a ten-minute block's nBits on the
    // first one-minute blocks are more than eight times too hard.
    assert!(refused_after_the_switch > 0);
    for height in [0, 1, chipnet.anchor_height, chipnet.anchor_height + 1] {
        for bits in [0x1a1a_55f5, BCH_POW_LIMIT_BITS, 0, 0x0480_0001, 0x1d01_0000] {
            let bits = CompactTarget::from_consensus(bits);
            assert_eq!(
                chipnet
                    .check_header_bits(bits, 1_794_745_996, u64::from(height), None)
                    .is_ok(),
                safa.accepts(bits, 1_794_745_996, height),
                "height {height} with {:#010x}",
                bits.to_consensus()
            );
        }
    }
}

/// Mainnet's switch block is not pinned yet. Pinned in both, the gate and
/// the covenant count the same ticks there too, on mainnet's narrow band
/// (half to 1.05 times): the last ten-minute header, the first with a
/// 60-tick interval, the first on a 60-tick parent and later ones, on
/// schedule and off it.
#[test]
fn a_pinned_mainnet_switch_gives_the_gate_and_the_safa_covenant_one_band() {
    let switch_height = 1_000_000;
    let gate = BchAsert {
        tick_schedule: BchAsert::MAINNET
            .tick_schedule
            .with_switch(IntervalSwitch {
                height: switch_height,
                interval: 60,
            })
            .unwrap(),
        ..BchAsert::MAINNET
    };
    let safa = SafaBand::new(p2poolv2_safa::NetworkParameters {
        interval_switch: Some(p2poolv2_safa::IntervalSwitch {
            height: switch_height,
            interval: 60,
        }),
        ..p2poolv2_safa::network_parameters("mainnet").unwrap()
    });
    // A header dated `late` seconds after the ASERT schedule of its parent.
    let dated = |height: u32, late: i64| {
        (gate.anchor_parent_time + gate.tick_schedule.ticks(height - 1)
            - i64::from(gate.anchor_height) * BCH_BLOCK_SPACING
            + BCH_BLOCK_SPACING
            + late) as u32
    };
    let mut checked = 0;
    for height in [
        switch_height,
        switch_height + 1,
        switch_height + 2,
        switch_height + 11,
        switch_height + 50_000,
    ] {
        for late in [-86_400, -7_200, 0, 600, 172_800] {
            checked += gate_and_covenant_agree(&gate, &safa, height, dated(height, late));
        }
    }
    assert!(checked > 500, "{checked}");
    // On schedule the first one-minute header's target is ten times the
    // anchor's, in both.
    let first = dated(switch_height + 1, 0);
    let ten_times = get_compact(set_compact(gate.anchor_bits).unwrap() * U256::from(10u64));
    assert_eq!(gate.next_bits_at(switch_height, first), Some(ten_times));
    assert!(safa.accepts(ten_times, first, switch_height + 1));
    assert!(!safa.accepts(
        CompactTarget::from_consensus(gate.anchor_bits),
        first,
        switch_height + 1
    ));
    // Unpinned, as mainnet is today, both take the anchor's own target there.
    let unpinned = SafaBand::for_network("mainnet");
    assert!(unpinned.accepts(
        CompactTarget::from_consensus(gate.anchor_bits),
        first,
        switch_height + 1
    ));
    assert!(
        BchAsert::MAINNET
            .check_header_bits(
                CompactTarget::from_consensus(gate.anchor_bits),
                first,
                u64::from(switch_height) + 1,
                None
            )
            .is_ok()
    );
}

/// Mainnet's easy side covers two hours of backwards skew too: a header
/// two hours before its parent computes a target 2^(7200 / 172800) times
/// too hard, under 1.05.
#[test]
fn real_mainnet_header_two_hours_before_its_parent_passes() {
    for (parent_height, parent_hex, child_hex) in MAINNET_PAIRS {
        let (parent, child) = real_pair(parent_hex, child_hex);
        BchAsert::MAINNET
            .check_header_bits(
                child.bits,
                parent.time - 7_200,
                u64::from(parent_height) + 1,
                None,
            )
            .unwrap_or_else(|error| panic!("block {}: {error}", parent_height + 1));
    }
}

#[test]
fn header_check_refuses_heights_below_the_anchor_and_invalid_bits() {
    let mainnet = BchAsert::MAINNET;
    let time = mainnet.anchor_parent_time as u32;
    let bits = CompactTarget::from_consensus(mainnet.anchor_bits);
    assert!(mainnet.check_header_bits(bits, time, 0, None).is_err());
    assert!(
        mainnet
            .check_header_bits(bits, time, u64::from(mainnet.anchor_height), None)
            .unwrap_err()
            .contains("anchor")
    );
    // Zero, negative, overflowing, and above the limit.
    for invalid in [
        0x0000_0000,
        0x0480_0001,
        0x2300_0001,
        0x2100_ffff,
        0x1d01_0000,
    ] {
        assert!(
            mainnet
                .check_header_bits(
                    CompactTarget::from_consensus(invalid),
                    time,
                    u64::from(mainnet.anchor_height) + 1,
                    None
                )
                .is_err(),
            "{invalid:#x}"
        );
    }
    assert_eq!(BchAsert::for_network(BitcoinCashNetwork::Regtest), None);
    assert_eq!(
        BchAsert::for_network(BitcoinCashNetwork::Chipnet),
        Some(BchAsert {
            tick_schedule: TickSchedule::for_network(BitcoinCashNetwork::Chipnet),
            ..BchAsert::CHIPNET
        })
    );
    // Testnet4 shares chipnet's anchor, not its activation.
    assert_eq!(
        BchAsert {
            tick_schedule: BchAsert::CHIPNET.tick_schedule,
            ..BchAsert::TESTNET4
        },
        BchAsert::CHIPNET
    );
}

#[test]
fn compact_round_trips_as_bchn_does() {
    for bits in [
        0x1804_dafe,
        0x1d00_ffff,
        0x0101_0000,
        0x1a2b_3c4d,
        0x0300_8000,
    ] {
        assert_eq!(get_compact(set_compact(bits).unwrap()).to_consensus(), bits);
    }
    assert_eq!(
        get_compact(bch_pow_limit()).to_consensus(),
        BCH_POW_LIMIT_BITS
    );
    // A negative or zero value has no target.
    assert_eq!(set_compact(0x0480_0001), None);
    assert_eq!(set_compact(0x0400_0000), None);
}

#[test]
fn bip34_heights_parse_as_bchn_writes_them() {
    assert_eq!(bip34_height(&[0x03, 0x15, 0xf9, 0x04, 0x07]), Some(325_909));
    assert_eq!(bip34_height(&[0x51]), Some(1));
    assert_eq!(bip34_height(&[0x60]), Some(16));
    assert_eq!(bip34_height(&[0x01, 0x11]), Some(17));
    assert_eq!(bip34_height(&[0x02, 0x80, 0x00]), Some(128));
    // Not minimal, negative, truncated, or not a height push.
    assert_eq!(bip34_height(&[0x02, 0x11, 0x00]), None);
    assert_eq!(bip34_height(&[0x01, 0x81]), None);
    assert_eq!(bip34_height(&[0x03, 0x15, 0xf9]), None);
    assert_eq!(bip34_height(&[0x00]), None);
    assert_eq!(bip34_height(&[]), None);
}

/// CHIP-2025-03's ASERT vectors (`chip202503/README.md`).
const CHIP_ASERT_VECTORS: &str = include_str!("chip202503/asert.json");

/// CHIP-2025-03's tick vectors.
const CHIP_TICK_VECTORS: &str = include_str!("chip202503/ticks.json");

fn hex_target(value: &serde_json::Value) -> U256 {
    U256::from_str_radix(value.as_str().unwrap(), 16).unwrap()
}

fn chip_bits(value: &serde_json::Value) -> u32 {
    parse_bits(value.as_str().unwrap())
}

fn maybe_number(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

fn number(value: &serde_json::Value) -> i64 {
    maybe_number(value).unwrap()
}

/// Every CHIP ASERT vector, the 600-tick golden ones, their 60-tick
/// siblings ten times easier, and the interval-ratio guard: the target
/// and the nBits match exactly.
#[test]
fn every_chip_asert_vector_matches() {
    let vectors: serde_json::Value = serde_json::from_str(CHIP_ASERT_VECTORS).unwrap();
    let pow_limit = hex_target(&vectors["powLimit"]);
    assert_eq!(pow_limit, bch_pow_limit());
    let rows = vectors["vectors"].as_array().unwrap();
    assert_eq!(rows.len(), 45);
    for row in rows {
        let name = row["name"].as_str().unwrap();
        let target = calculate_asert(
            hex_target(&row["refTarget"]),
            number(&row["nTimeDiff"]),
            number(&row["tickDelta"]),
            number(&row["anchorIdealBlockTime"]),
            number(&row["nextIdealBlockTime"]),
            number(&row["nHalfLife"]),
            pow_limit,
        )
        .unwrap_or_else(|| panic!("{name} has no target"));
        assert_eq!(target, hex_target(&row["expectedTarget"]), "{name}");
        assert_eq!(
            get_compact(target).to_consensus(),
            chip_bits(&row["expectednBits"]),
            "{name}"
        );
    }
}

/// The CHIP's end-to-end rows: the test networks' minimum-difficulty rule
/// at more than twice the new block's interval (1,200 s before
/// activation, 120 s after), and the tick difference from the anchor's
/// own ticks.
#[test]
fn every_chip_asert_wrapper_row_matches() {
    let vectors: serde_json::Value = serde_json::from_str(CHIP_ASERT_VECTORS).unwrap();
    let rows = vectors["wrapper"].as_array().unwrap();
    assert_eq!(rows.len(), 10);
    for row in rows {
        let name = row["name"].as_str().unwrap();
        let anchor = &row["anchor"];
        let next_interval = number(&row["nextIdealBlockTime"]);
        let prev_time = number(&row["nPrevBlockTime"]);
        let bits = if row["fPowAllowMinDifficultyBlocks"].as_bool().unwrap()
            && number(&row["nNextBlockTime"]) > prev_time + 2 * next_interval
        {
            BCH_POW_LIMIT_BITS
        } else {
            let target = calculate_asert(
                set_compact(chip_bits(&anchor["nBits"])).unwrap(),
                prev_time - number(&anchor["nPrevBlockTime"]),
                number(&row["nPrevTick"]) - number(&anchor["nTick"]),
                number(&anchor["nBlockIntervalTime"]),
                next_interval,
                number(&row["nASERTHalfLife"]),
                bch_pow_limit(),
            )
            .unwrap_or_else(|| panic!("{name} has no target"));
            get_compact(target).to_consensus()
        };
        assert_eq!(bits, chip_bits(&row["expectednBits"]), "{name}");
    }
}

/// A schedule from a CHIP vector's entries, or the error that refuses it.
/// A height past `u32` is refused as well: no Bitcoin Cash height gets
/// there.
fn chip_schedule(entries: &[serde_json::Value]) -> Option<TickSchedule> {
    entries
        .iter()
        .try_fold(TickSchedule::TEN_MINUTES, |schedule, entry| {
            let height =
                maybe_number(&entry["height"]).and_then(|height| u32::try_from(height).ok())?;
            let interval = maybe_number(&entry["tau"])?;
            schedule
                .with_switch(IntervalSwitch { height, interval })
                .ok()
        })
}

/// Every CHIP tick vector, for every schedule, including several switches
/// (to 300, back to 600, to 60 and to 600 again): ticks from a height, the
/// height from ticks, and each block's interval. Values past `u32` heights
/// are not used here.
#[test]
fn every_chip_tick_vector_matches() {
    let vectors: serde_json::Value = serde_json::from_str(CHIP_TICK_VECTORS).unwrap();
    let mut checked = 0;
    for schedule in vectors["schedules"].as_array().unwrap() {
        let name = schedule["name"].as_str().unwrap();
        let ticks = chip_schedule(schedule["schedule"].as_array().unwrap())
            .unwrap_or_else(|| panic!("{name} was refused"));
        for query in schedule["queries"].as_array().into_iter().flatten() {
            let (Some(arg), Some(expected)) = (
                maybe_number(&query["arg"]),
                maybe_number(&query["expected"]),
            ) else {
                continue;
            };
            let height = u32::try_from(arg);
            let actual = match (query["op"].as_str().unwrap(), height) {
                ("HeightToTick", Ok(height)) => ticks.ticks(height),
                ("GetBlockIntervalTime", Ok(height)) => ticks.interval_of(height),
                ("GetNextBlockIntervalTime", Ok(height)) => ticks.interval_after(height),
                ("TickToHeight", _) if u32::try_from(expected).is_ok() => {
                    i64::from(ticks.height_at_or_below(arg))
                }
                _ => continue,
            };
            assert_eq!(actual, expected, "{name}: {query}");
            checked += 1;
        }
    }
    assert!(checked > 60, "only {checked} tick queries checked");
}

/// The CHIP's schedule validation: intervals that do not divide 600, out
/// of order or on a height off the 600-tick grid are refused, the rest
/// accepted.
#[test]
fn chip_schedule_validation_matches() {
    let vectors: serde_json::Value = serde_json::from_str(CHIP_TICK_VECTORS).unwrap();
    let rows = vectors["validation"].as_array().unwrap();
    assert!(rows.len() >= 10);
    let mut past_u32 = 0;
    for row in rows {
        let name = row["name"].as_str().unwrap();
        // Heights past `u32`, which no Bitcoin Cash chain reaches, are
        // outside this schedule's range whatever the CHIP says of them.
        let entries = row["schedule"].as_array().unwrap();
        if entries.iter().any(|entry| {
            maybe_number(&entry["height"]).is_none_or(|height| u32::try_from(height).is_err())
        }) {
            past_u32 += 1;
            continue;
        }
        let accepted = chip_schedule(row["schedule"].as_array().unwrap()).is_some();
        assert_eq!(accepted, row["accept"].as_bool().unwrap(), "{name}");
    }
    assert_eq!(past_u32, 5);
}

/// Chipnet's pinned ticks and intervals around its activation block,
/// 332,450.
#[test]
fn chipnet_switches_to_sixty_tick_blocks_after_block_332450() {
    let chipnet = TickSchedule::for_network(BitcoinCashNetwork::Chipnet);
    assert_eq!(chipnet.ticks(332_450), 332_450 * 600);
    assert_eq!(chipnet.ticks(332_451), 332_450 * 600 + 60);
    assert_eq!(chipnet.interval_after(332_449), 600);
    assert_eq!(chipnet.interval_after(332_450), 60);
    assert_eq!(BchAsert::CHIPNET.min_difficulty_gap_at(332_449), 1_200);
    assert_eq!(BchAsert::CHIPNET.min_difficulty_gap_at(332_450), 120);
    // Mainnet has no pin: by height every block is a 600-tick block.
    assert_eq!(
        TickSchedule::for_network(BitcoinCashNetwork::Mainnet).ticks(950_000),
        950_000 * 600
    );
    assert_eq!(
        TickSchedule::for_network(BitcoinCashNetwork::Mainnet).interval_after(950_000),
        600
    );
}

/// On schedule across the switch, the first one-minute block's target is
/// ten times the last ten-minute block's: the same work per second.
#[test]
fn the_first_one_minute_block_is_ten_times_easier_on_schedule() {
    let chipnet = BchAsert::CHIPNET;
    let last_slow = 332_449u32;
    // A parent dated exactly on the ASERT schedule keeps the anchor target.
    let on_schedule = |height: u32| {
        (chipnet.anchor_parent_time + chipnet.tick_schedule.ticks(height)
            - i64::from(chipnet.anchor_height) * BCH_BLOCK_SPACING
            + BCH_BLOCK_SPACING) as u32
    };
    let slow = chipnet
        .next_target_at(last_slow, on_schedule(last_slow))
        .unwrap();
    let fast = chipnet
        .next_target_at(last_slow + 1, on_schedule(last_slow + 1))
        .unwrap();
    assert_eq!(slow, set_compact(chipnet.anchor_bits).unwrap());
    assert_eq!(fast, bch_pow_limit());
    // The anchor is the limit on chipnet, so compare below it on mainnet's
    // numbers too: ten times, exactly, before the clamp.
    let reference = set_compact(0x1804_dafe).unwrap();
    let slow = calculate_asert(
        reference,
        600 * 1_001,
        600 * 1_000,
        600,
        600,
        172_800,
        bch_pow_limit(),
    );
    let fast = calculate_asert(
        reference,
        600 * 1_001,
        600 * 1_000,
        600,
        60,
        172_800,
        bch_pow_limit(),
    );
    assert_eq!(fast.unwrap(), slow.unwrap() * U256::from(10u64));
}

/// Real chipnet blocks around the one-minute activation, read from a Knuth
/// node that follows the upgraded chain (2026-11-15): height, nTime and
/// nBits. Block 332,450 is the last ten-minute block: its median time past,
/// 1,794,744,504, is the first at or after 1,794,744,000.
pub(crate) const CHIPNET_AROUND_ACTIVATION: [(u32, u32, u32); 22] = [
    (332_439, 1_794_740_864, 0x1a21_8c7e),
    (332_440, 1_794_741_541, 0x1a1f_4cf0),
    (332_441, 1_794_742_743, 0x1d00_ffff),
    (332_442, 1_794_742_920, 0x1a23_ac7c),
    (332_443, 1_794_743_094, 0x1a20_e1df),
    (332_444, 1_794_743_302, 0x1a1e_4b31),
    (332_445, 1_794_744_504, 0x1d00_ffff),
    (332_446, 1_794_744_756, 0x1a1f_8ac0),
    (332_447, 1_794_745_241, 0x1a1d_8002),
    (332_448, 1_794_745_658, 0x1a1c_dab3),
    (332_449, 1_794_745_967, 0x1a1b_dac4),
    (332_450, 1_794_745_996, 0x1a1a_55f5),
    (332_451, 1_794_746_156, 0x1d00_ffff),
    (332_452, 1_794_746_219, 0x1b00_f080),
    (332_453, 1_794_746_263, 0x1b00_f0a4),
    (332_454, 1_794_746_387, 0x1d00_ffff),
    (332_455, 1_794_746_301, 0x1b00_f2e0),
    (332_456, 1_794_746_372, 0x1b00_ec23),
    (332_457, 1_794_746_406, 0x1b00_eca3),
    (332_458, 1_794_746_447, 0x1b00_eb75),
    (332_459, 1_794_746_537, 0x1b00_ea99),
    (332_460, 1_794_746_573, 0x1b00_ebf4),
];

/// Every chipnet block around the activation carries the nBits P2Pool
/// computes from its parent: ASERT ten times easier from block 332,451 on,
/// and the proof-of-work limit after a gap of more than 20 minutes before
/// it (332,441 and 332,445) and of more than 2 minutes after it (332,451
/// and 332,454).
#[test]
fn real_chipnet_blocks_around_the_activation_carry_the_bits_computed_for_them() {
    let chipnet = BchAsert::CHIPNET;
    let mut minimum_difficulty = Vec::with_capacity(4);
    for pair in CHIPNET_AROUND_ACTIVATION.windows(2) {
        let [(parent_height, parent_time, _), (height, time, bits)] = pair else {
            unreachable!()
        };
        let expected = if *time > parent_time + chipnet.min_difficulty_gap_at(*parent_height) {
            minimum_difficulty.push(*height);
            BCH_POW_LIMIT_BITS
        } else {
            chipnet
                .next_bits_at(*parent_height, *parent_time)
                .unwrap()
                .to_consensus()
        };
        assert_eq!(expected, *bits, "block {height}");
    }
    assert_eq!(minimum_difficulty, [332_441, 332_445, 332_451, 332_454]);
    // Knuth's own ticks for the activation block and the next.
    assert_eq!(chipnet.tick_schedule.ticks(332_450), 199_470_000);
    assert_eq!(chipnet.tick_schedule.ticks(332_460), 199_470_600);
}

/// A Bitcoin Cash lock height counts ten-minute units: on chipnet a lock
/// below the activation block opens at the next block, and one past it ten
/// one-minute blocks per unit later. Mainnet has not activated.
#[test]
fn a_lock_height_opens_ten_one_minute_blocks_per_unit_after_activation() {
    let chipnet = TickSchedule::for_network(BitcoinCashNetwork::Chipnet);
    assert_eq!(chipnet.first_block_after_lock_height(332_000), 332_001);
    assert_eq!(chipnet.first_block_after_lock_height(332_449), 332_450);
    assert_eq!(chipnet.first_block_after_lock_height(332_450), 332_451);
    assert_eq!(chipnet.first_block_after_lock_height(332_451), 332_461);
    assert_eq!(chipnet.first_block_after_lock_height(332_594), 333_891);
    // The block it names is the first whose ticks pass 600 times the lock.
    for lock in [332_449u32, 332_450, 332_451, 332_594] {
        let first = chipnet.first_block_after_lock_height(lock);
        assert!(chipnet.ticks(first) > 600 * i64::from(lock), "{lock}");
        assert!(chipnet.ticks(first - 1) <= 600 * i64::from(lock), "{lock}");
    }
    assert_eq!(
        TickSchedule::for_network(BitcoinCashNetwork::Mainnet)
            .first_block_after_lock_height(950_000),
        950_001
    );
}

/// A later switch pinned to a block, to 30-second blocks at height 380,000
/// on chipnet, counts in ticks like the first: a 30-second block's target is
/// twice a one-minute block's, and coinbase maturity stays 60,000 ticks,
/// 2,000 thirty-second blocks.
#[test]
fn a_later_switch_to_thirty_seconds_counts_in_ticks() {
    let chipnet = TickSchedule::for_network(BitcoinCashNetwork::Chipnet);
    let at_switch = chipnet.ticks(380_000);
    assert_eq!(at_switch % 600, 0);
    let switch = IntervalSwitch {
        height: 380_000,
        interval: 30,
    };
    let followed = chipnet.with_switch(switch).unwrap();
    assert_eq!(followed.ticks(400_000), at_switch + 20_000 * 30);
    assert_eq!(followed.interval_after(400_000), 30);
    let reference = set_compact(0x1804_dafe).unwrap();
    let one_minute = calculate_asert(
        reference,
        600 * 1_001,
        600 * 1_000,
        600,
        60,
        172_800,
        bch_pow_limit(),
    )
    .unwrap();
    let thirty_seconds = calculate_asert(
        reference,
        600 * 1_001,
        600 * 1_000,
        600,
        30,
        172_800,
        bch_pow_limit(),
    )
    .unwrap();
    assert_eq!(thirty_seconds, one_minute * U256::from(2u64));
    assert!(!followed.coinbase_spendable_in_next_block(400_000, 1_999));
    assert!(followed.coinbase_spendable_in_next_block(400_000, 2_000));
    // A switch off the 600-tick grid, or to an interval the CHIP refuses, is
    // refused.
    assert_eq!(
        followed.with_switch(IntervalSwitch {
            height: 400_001,
            interval: 60,
        }),
        Err(ScheduleError::Grid {
            height: 400_001,
            ticks: followed.ticks(400_001),
        })
    );
    assert_eq!(
        chipnet.with_switch(IntervalSwitch {
            height: 380_000,
            interval: 7,
        }),
        Err(ScheduleError::Interval(7))
    );
}

/// The ten chipnet blocks before [`CHIPNET_AROUND_ACTIVATION`]: height and
/// nTime, from the same Knuth node. With them every block from 332,439 on
/// has its ten ancestors.
pub(crate) const CHIPNET_BEFORE_ACTIVATION: [(u32, u32); 10] = [
    (332_429, 1_794_735_255),
    (332_430, 1_794_736_345),
    (332_431, 1_794_736_878),
    (332_432, 1_794_737_463),
    (332_433, 1_794_737_555),
    (332_434, 1_794_738_496),
    (332_435, 1_794_738_670),
    (332_436, 1_794_739_872),
    (332_437, 1_794_739_926),
    (332_438, 1_794_740_624),
];

/// Chipnet's real blocks from 332,429 to 332,460, height and nTime, oldest
/// first.
pub(crate) fn chipnet_times_around_activation() -> Vec<(u32, u32)> {
    CHIPNET_BEFORE_ACTIVATION
        .into_iter()
        .chain(
            CHIPNET_AROUND_ACTIVATION
                .iter()
                .map(|(height, time, _)| (*height, *time)),
        )
        .collect()
}

/// From block times alone, the timed rule makes 332,450 chipnet's last
/// ten-minute block: the first whose median time past reaches 2026-11-15
/// 12:00 UTC. It gives every block from 332,439 to 332,460 the next interval
/// chipnet's pin gives it, so the pin is a shortcut, not another rule.
#[test]
fn the_timed_rule_switches_chipnet_at_its_pinned_block() {
    let chipnet = TickSchedule::for_network(BitcoinCashNetwork::Chipnet);
    let times = chipnet_times_around_activation();
    let mut first_sixty = None;
    for (index, (height, _)) in times.iter().enumerate().skip(10) {
        // The block's own time and its ten ancestors', newest first.
        let window: Vec<u32> = times[index - 10..=index]
            .iter()
            .rev()
            .map(|(_, time)| *time)
            .collect();
        let next_interval = chipnet.next_interval(*height, &window);
        assert_eq!(
            next_interval,
            chipnet.interval_after(*height),
            "block {height}"
        );
        if next_interval == 60 && first_sixty.is_none() {
            first_sixty = Some(*height);
        }
    }
    assert_eq!(first_sixty, Some(332_450));
    // The median times past either side of the activation time.
    let median = |height: u32| {
        let index = times.iter().position(|(at, _)| *at == height).unwrap();
        let window: Vec<u32> = times[index - 10..=index]
            .iter()
            .map(|(_, time)| *time)
            .collect();
        median_time_past(&window).unwrap()
    };
    assert_eq!(median(332_449), 1_794_743_302);
    assert_eq!(median(332_450), 1_794_744_504);
}

/// BCHN's `GetMedianTimePast`: the middle of the sorted times, the upper
/// middle of an even count, as near the chain's first block.
#[test]
fn median_time_past_is_bchns() {
    assert_eq!(median_time_past(&[]), None);
    assert_eq!(median_time_past(&[7]), Some(7));
    assert_eq!(median_time_past(&[9, 1]), Some(9));
    assert_eq!(median_time_past(&[5, 1, 9]), Some(5));
    assert_eq!(median_time_past(&[11, 3, 7, 1]), Some(7));
    let eleven = [10, 1, 9, 2, 8, 3, 7, 4, 6, 5, 11];
    assert_eq!(median_time_past(&eleven), Some(6));
}

/// A block with fewer than its ten ancestors' times known takes the pins'
/// next interval; one near the chain's first block has every ancestor it can
/// and uses its median time past.
#[test]
fn a_median_time_past_needs_ten_ancestors_or_the_chains_first_block() {
    let mainnet = TickSchedule::for_network(BitcoinCashNetwork::Mainnet);
    let after = 1_810_382_400_u32;
    // Ten times past the activation are not enough at height 1,000,000: the
    // pins, which have no mainnet switch, answer 600.
    assert_eq!(mainnet.next_interval(1_000_000, &[after; 10]), 600);
    assert_eq!(mainnet.next_interval(1_000_000, &[after; 11]), 60);
    // Extra times are not read.
    let mut twelve = vec![after; 6];
    twelve.extend([0; 6]);
    assert_eq!(mainnet.next_interval(1_000_000, &twelve), 60);
    twelve.swap(5, 11);
    assert_eq!(mainnet.next_interval(1_000_000, &twelve), 600);
    // Block 3 reads itself and its three ancestors.
    assert_eq!(mainnet.next_interval(3, &[after; 4]), 60);
    assert_eq!(mainnet.next_interval(3, &[after; 3]), 600);
}

/// Every network's timed switches are ones the CHIP allows, in activation
/// order: mainnet's (and testnet's, scalenet's and regtest's, as in BCHN) at
/// 2027-05-15 12:00 UTC and chipnet's at 2026-11-15 12:00 UTC, both to 60
/// ticks. A switch from ten-minute blocks lands on the 600-tick grid by
/// construction.
#[test]
fn every_networks_timed_switches_are_allowed() {
    for network in [
        BitcoinCashNetwork::Mainnet,
        BitcoinCashNetwork::Chipnet,
        BitcoinCashNetwork::Regtest,
        BitcoinCashNetwork::Scalenet,
    ] {
        let schedule = TickSchedule::for_network(network);
        let timed = schedule.timed_switches();
        assert!(!timed.is_empty(), "{network:?}");
        assert!(
            timed
                .iter()
                .all(|switch| divides_600(switch.interval) && switch.interval != 600),
            "{network:?}"
        );
        assert!(
            timed
                .windows(2)
                .all(|pair| pair[0].activation_time < pair[1].activation_time),
            "{network:?}"
        );
        // Every pin follows a timed switch to its interval.
        for pin in schedule.switches() {
            assert!(
                timed.iter().any(|switch| switch.interval == pin.interval),
                "{network:?}"
            );
        }
    }
    assert_eq!(
        TickSchedule::for_network(BitcoinCashNetwork::Mainnet).timed_switches(),
        [TimedSwitch {
            activation_time: 1_810_382_400,
            interval: 60,
        }]
    );
    assert_eq!(
        TickSchedule::for_network(BitcoinCashNetwork::Chipnet).timed_switches(),
        [TimedSwitch {
            activation_time: 1_794_744_000,
            interval: 60,
        }]
    );
    let mainnet = TickSchedule::for_network(BitcoinCashNetwork::Mainnet);
    assert_eq!(mainnet.interval_reached(1_810_382_399), 600);
    assert_eq!(mainnet.interval_reached(1_810_382_400), 60);
    assert_eq!(TickSchedule::TEN_MINUTES.interval_reached(i64::MAX), 600);
}

/// BCHN's `CheckTxInputs`: a coinbase can be spent in the block built on a
/// tip when that block's ticks, the tip's plus its next interval, are 60,000
/// past the coinbase's block. Across chipnet's switch: a coinbase in
/// 332,400 (ticks 199,440,000) needs the 50 ten-minute blocks to 332,450 and
/// 500 one-minute blocks after it.
#[test]
fn coinbase_maturity_counts_the_next_blocks_ticks() {
    let coin_ticks = 332_400 * 600;
    let tip = |ticks: i64| TickInfo {
        ticks,
        next_interval: 60,
    };
    // The block on 332,949 has ticks 199,470,000 + 500 * 60.
    assert!(!tip(199_470_000 + 498 * 60).coinbase_spendable_in_next_block(coin_ticks));
    assert!(tip(199_470_000 + 499 * 60).coinbase_spendable_in_next_block(coin_ticks));
    let chipnet = TickSchedule::for_network(BitcoinCashNetwork::Chipnet);
    assert!(!chipnet.coinbase_spendable_in_next_block(332_400, 549));
    assert!(chipnet.coinbase_spendable_in_next_block(332_400, 550));
    assert_eq!(
        chipnet.tick_info(332_948).next_ticks(),
        199_470_000 + 499 * 60
    );
}
