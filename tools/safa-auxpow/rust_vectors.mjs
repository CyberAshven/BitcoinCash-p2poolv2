// The golden vectors P2Pool's Rust side already checks, read from its own
// test sources so the covenant is held to exactly the same bytes: Pickaxe's
// merge-mining v2 vectors (PR #70), BCHN's aserti3-2d specification runs and
// real mainnet and chipnet header pairs.
import { readFileSync } from 'node:fs';

const root = new URL('../../p2poolv2_lib/src/shares/', import.meta.url);

function rustSource(path) {
  return readFileSync(new URL(path, root), 'utf8');
}

/** Every `const NAME: &str = "..."` or `concat!("...", ...)` in a Rust file. */
function stringConstants(source) {
  const constants = {};
  const pattern = /const (\w+): &str =\s*(concat!\(([\s\S]*?)\)|"([^"]*)");/g;
  for (const match of source.matchAll(pattern)) {
    const parts = match[3] === undefined ? [match[4]] : [...match[3].matchAll(/"([^"]*)"/g)].map(part => part[1]);
    constants[match[1]] = parts.join('');
  }
  return constants;
}

function headerPairs(source, name) {
  const start = source.indexOf(`const ${name}:`);
  if (start < 0) throw new Error(`no ${name} in the Rust ASERT tests`);
  const body = source.slice(start, source.indexOf('];', start));
  return [...body.matchAll(/\(\s*(\d+),\s*"([0-9a-f]+)",\s*"([0-9a-f]+)",?\s*\)/g)]
    .map(match => ({ parentHeight: Number(match[1]), parent: match[2], child: match[3] }));
}

export const auxpowVectors = stringConstants(rustSource('auxpow/tests.rs'));

const asertTests = rustSource('bch_asert/tests.rs');
export const mainnetPairs = headerPairs(asertTests, 'MAINNET_PAIRS');
export const chipnetPairs = headerPairs(asertTests, 'CHIPNET_PAIRS');

/** One aserti3-2d run: its anchor and every (height, time, expected bits) line. */
export function asertRun(name) {
  const text = rustSource(`bch_asert/aserti3-2d/${name}`);
  const parameter = key => {
    const line = text.split('\n').find(row => row.startsWith(`##   ${key}: `));
    if (line === undefined) throw new Error(`${name} has no ${key}`);
    return line.slice(`##   ${key}: `.length).trim();
  };
  const lines = text.split('\n').filter(line => line.trim() !== '' && !line.startsWith('#'))
    .map(line => {
      const fields = line.trim().split(/\s+/);
      return { height: BigInt(fields[1]), time: BigInt(fields[2]), bits: Number.parseInt(fields[3], 16) };
    });
  return {
    anchorHeight: BigInt(parameter('anchor height')),
    anchorParentTime: BigInt(parameter('anchor parent time')),
    anchorBits: Number.parseInt(parameter('anchor nBits'), 16),
    iterations: Number(parameter('iterations')),
    lines,
  };
}

export const ASERT_RUNS = Array.from({ length: 12 }, (_, index) => `run${String(index + 1).padStart(2, '0')}`);
