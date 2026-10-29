use super::*;
use bitcoin::block::Header;
use bitcoin::consensus::deserialize;

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
                i64::try_from(height - anchor_height).unwrap(),
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
const MAINNET_PAIRS: [(u32, &str, &str); 9] = [
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
const CHIPNET_PAIRS: [(u32, &str, &str); 9] = [
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

fn header(hex_header: &str) -> Header {
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
            BchAsert::MAINNET.next_bits(parent_height, parent.time),
            Some(child.bits),
            "block {}",
            parent_height + 1
        );
        // The header check, which has only the child, accepts it.
        BchAsert::MAINNET
            .check_header_bits(child.bits, child.time, u64::from(parent_height) + 1)
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
                .next_bits(parent_height, parent.time)
                .unwrap()
        };
        assert_eq!(child.bits, expected, "block {}", parent_height + 1);
        BchAsert::CHIPNET
            .check_header_bits(child.bits, child.time, u64::from(parent_height) + 1)
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
            .next_bits(
                mainnet.anchor_height,
                (mainnet.anchor_parent_time + BCH_BLOCK_SPACING) as u32
            )
            .unwrap()
            .to_consensus(),
        mainnet.anchor_bits
    );
    assert_eq!(mainnet.next_bits(mainnet.anchor_height - 1, u32::MAX), None);
    // Far behind schedule it stays at the limit; far ahead it never reaches 0.
    let chipnet = BchAsert::CHIPNET;
    assert_eq!(
        chipnet
            .next_bits(chipnet.anchor_height + 1, u32::MAX)
            .unwrap()
            .to_consensus(),
        BCH_POW_LIMIT_BITS
    );
    assert!(!chipnet.next_target(u32::MAX, 0).unwrap().is_zero());
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
    let check =
        |bits: u32| mainnet.check_header_bits(CompactTarget::from_consensus(bits), time, height);
    check(0x1804_dafe).unwrap();
    check(0x1802_6d7f).unwrap();
    assert!(check(0x1802_6d7e).unwrap_err().contains("harder"));
    check(0x1805_1924).unwrap();
    assert!(check(0x1805_1925).unwrap_err().contains("easier"));
    // Mainnet allows no minimum-difficulty block.
    assert!(check(BCH_POW_LIMIT_BITS).is_err());
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
    let computed = chipnet.next_target(height as u32 - 1, time).unwrap();
    let check = |target: U256| chipnet.check_header_bits(get_compact(target), time, height);
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
        )
        .unwrap();
}

/// The exact `computed * 8` edge, where ASERT clamps to 1 so the edge and
/// one above it are both exact compact values (`tools/safa-auxpow`'s
/// `asert.test.mjs` checks the same two bits).
#[test]
fn test_network_easy_edge_is_exactly_eight_times() {
    let chipnet = BchAsert::CHIPNET;
    let height = u64::from(chipnet.anchor_height) + 10_001;
    let time = (chipnet.anchor_parent_time + BCH_BLOCK_SPACING) as u32;
    assert_eq!(
        chipnet.next_target(height as u32 - 1, time),
        Some(U256::one())
    );
    assert_eq!(set_compact(0x0108_0000), Some(U256::from(8)));
    chipnet
        .check_header_bits(CompactTarget::from_consensus(0x0108_0000), time, height)
        .unwrap();
    assert_eq!(set_compact(0x0109_0000), Some(U256::from(9)));
    assert!(
        chipnet
            .check_header_bits(CompactTarget::from_consensus(0x0109_0000), time, height)
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
        BchAsert::CHIPNET.next_bits(parent_height, parent.time),
        Some(child.bits)
    );
    for seconds_before in [200, 260, 600, 3_600, 7_200] {
        BchAsert::CHIPNET
            .check_header_bits(child.bits, parent.time - seconds_before, height)
            .unwrap_or_else(|error| panic!("{seconds_before} s before: {error}"));
    }
    // The same two hours on mainnet's band would need 2^(1/24), about
    // 1.029, which 1.05 covers.
    let mut old_band = BchAsert::CHIPNET;
    (old_band.easy_numerator, old_band.easy_denominator) = (105, 100);
    old_band
        .check_header_bits(child.bits, parent.time - 200, height)
        .unwrap();
    assert!(
        old_band
            .check_header_bits(child.bits, parent.time - 260, height)
            .is_err()
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
            )
            .unwrap_or_else(|error| panic!("block {}: {error}", parent_height + 1));
    }
}

#[test]
fn header_check_refuses_heights_below_the_anchor_and_invalid_bits() {
    let mainnet = BchAsert::MAINNET;
    let time = mainnet.anchor_parent_time as u32;
    let bits = CompactTarget::from_consensus(mainnet.anchor_bits);
    assert!(mainnet.check_header_bits(bits, time, 0).is_err());
    assert!(
        mainnet
            .check_header_bits(bits, time, u64::from(mainnet.anchor_height))
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
                    u64::from(mainnet.anchor_height) + 1
                )
                .is_err(),
            "{invalid:#x}"
        );
    }
    assert_eq!(BchAsert::for_network(BitcoinCashNetwork::Regtest), None);
    assert_eq!(
        BchAsert::for_network(BitcoinCashNetwork::Chipnet),
        Some(BchAsert::TESTNET4)
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
