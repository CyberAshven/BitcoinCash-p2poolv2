// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Payout levels and the draw, as section 4 of the design page gives them.
//!
//! A payee whose amount in a block is below its level is pooled with the
//! others below the same level. The group's pot `S` is cut into `n = S / L`
//! parts of `D = S / n` satoshis (both rounded down), and `n` points spaced
//! `D` apart from an offset `seed mod D` each pay `D` to the payee whose
//! running total first exceeds the point. The first point's payee also gets
//! the remainder `S - n * D`. With `n` = 0 one point `seed mod S` picks a
//! payee for the whole pot. The pot is never changed, only how it is split.

use crate::constants::DUST_LEVEL_SATOSHIS;

/// Satoshis of payout level `k`: `546 * 2^k`.
pub fn level_amount(level: u32) -> u64 {
    DUST_LEVEL_SATOSHIS << level
}

/// The draw over one level group. `amounts` are the payees' amounts in their
/// order (by locking script bytes on the chain), `level` the group's level
/// in satoshis and `seed` the draw's seed. Returns each payee's winnings, in
/// the same order; their sum is the group's pot.
pub fn draw_level_group(amounts: &[u64], level: u64, seed: u64) -> Vec<u64> {
    let pot: u64 = amounts.iter().sum();
    let mut winnings = vec![0; amounts.len()];
    if pot > 0 {
        let parts = pot / level;
        match pot.checked_div(parts) {
            None => {
                let winner = payee_covering(amounts, seed % pot);
                winnings[winner] = pot;
            }
            Some(spacing) => {
                let offset = seed % spacing;
                let mut start = 0;
                amounts
                    .iter()
                    .zip(winnings.iter_mut())
                    .for_each(|(amount, won)| {
                        let end = start + amount;
                        let points = points_below(end, offset, spacing, parts)
                            - points_below(start, offset, spacing, parts);
                        *won = points * spacing;
                        start = end;
                    });
                let first = payee_covering(amounts, offset);
                winnings[first] += pot - parts * spacing;
            }
        }
    }
    winnings
}

/// How many of the points `offset + j * spacing`, `j` = 0 to `parts - 1`,
/// lie below `bound`.
fn points_below(bound: u64, offset: u64, spacing: u64, parts: u64) -> u64 {
    if bound <= offset {
        0
    } else {
        ((bound - offset - 1) / spacing + 1).min(parts)
    }
}

/// The payee whose running total of amounts first exceeds `point`. The
/// caller keeps `point` below the pot, so there is one.
fn payee_covering(amounts: &[u64], point: u64) -> usize {
    let mut running = 0;
    amounts
        .iter()
        .position(|amount| {
            running += amount;
            running > point
        })
        .unwrap_or(amounts.len() - 1)
}

#[cfg(test)]
mod tests;
