//! Collect every Orca Whirlpool swap from a Yellowstone gRPC (Dragon's Mouth)
//! stream and print one JSON line per swap leg.
//!
//! The parser is generated at compile time from the Whirlpool Codama IDL, so
//! its prefilter already narrows the subscription to the Orca program
//! (`whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc`). We handle every swap
//! variant: `swap`, `swapV2`, `twoHopSwap`, `twoHopSwapV2` (two-hop emits one
//! leg per pool).
//!
//! Real in/out amounts and mints are read from the transaction's pre/post
//! token balances on the pool vaults — the instruction args only carry the
//! *specified* amount, not both executed sides.

use std::{collections::HashMap, path::PathBuf};

use clap::Parser as _;
use serde::Serialize;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use yellowstone_vixen::{self as vixen, Pipeline};
use yellowstone_vixen_core::{instruction::InstructionUpdate, Pubkey};
use yellowstone_vixen_proc_macro::include_vixen_parser;
use yellowstone_vixen_yellowstone_grpc_source::YellowstoneGrpcSource;

include_vixen_parser!("whirlpool.json");

use whirlpool::instruction::Instruction as Ix;

#[derive(clap::Parser)]
#[command(version, author, about)]
struct Opts {
    /// Path to the Vixen TOML config (gRPC endpoint + x-token).
    #[arg(long, short, default_value = concat!(env!("CARGO_MANIFEST_DIR"), "/Vixen.toml"))]
    config: PathBuf,
}

/// One executed swap on a single Whirlpool pool.
///
/// Example output:
/// ```rust, ignore
/// {"slot":312345678,"program_id":"whirLbM...","pool":"HJPj...","kind":"swap",
///  "in_mint":"So111...112","in_amount":"1000000000","in_decimals":9,
///  "out_mint":"EPjF...Dt1v","out_amount":"152340000","out_decimals":6}
/// ```
#[derive(Debug, Serialize)]
struct SwapRecord {
    slot: u64,
    program_id: String,
    pool: String,
    kind: &'static str,
    in_mint: String,
    in_amount: String,
    in_decimals: u32,
    out_mint: String,
    out_amount: String,
    out_decimals: u32,
}

/// A pool vault's token amount at one point in time.
#[derive(Clone, Debug)]
struct VaultAmt {
    mint: String,
    decimals: u32,
    amount: u128,
}

/// `(mint, raw_amount, decimals)`.
type Side = (String, u128, u32);

/// Derive the `(input, output)` sides of a swap from a pool's two vault
/// balances before and after the transaction.
///
/// The pool *receives* the input token (its vault rises) and *sends* the
/// output token (its vault falls). Returns `None` when the vaults did not move
/// in opposite directions (no-op, or a non-swap touch of these accounts).
fn leg(a_pre: &VaultAmt, a_post: &VaultAmt, b_pre: &VaultAmt, b_post: &VaultAmt) -> Option<(Side, Side)> {
    let da = a_post.amount as i128 - a_pre.amount as i128;
    let db = b_post.amount as i128 - b_pre.amount as i128;

    if da > 0 && db < 0 {
        Some((
            (a_post.mint.clone(), da as u128, a_post.decimals),
            (b_post.mint.clone(), (-db) as u128, b_post.decimals),
        ))
    } else if db > 0 && da < 0 {
        Some((
            (b_post.mint.clone(), db as u128, b_post.decimals),
            (a_post.mint.clone(), (-da) as u128, a_post.decimals),
        ))
    } else {
        None
    }
}

/// Pool + its two vaults for one leg, tagged with the instruction variant.
type PoolLeg<'a> = (&'static str, &'a Pubkey, &'a Pubkey, &'a Pubkey);

fn swap_legs(ix: &Ix) -> Vec<PoolLeg<'_>> {
    match ix {
        Ix::Swap { accounts, .. } => {
            vec![("swap", &accounts.whirlpool, &accounts.token_vault_a, &accounts.token_vault_b)]
        },
        Ix::SwapV2 { accounts, .. } => {
            vec![("swapV2", &accounts.whirlpool, &accounts.token_vault_a, &accounts.token_vault_b)]
        },
        Ix::TwoHopSwap { accounts, .. } => vec![
            ("twoHopSwap.1", &accounts.whirlpool_one, &accounts.token_vault_one_a, &accounts.token_vault_one_b),
            ("twoHopSwap.2", &accounts.whirlpool_two, &accounts.token_vault_two_a, &accounts.token_vault_two_b),
        ],
        // V2 two-hop names vaults by role (input/intermediate/output) instead
        // of a/b. Direction is still resolved from the balance deltas.
        Ix::TwoHopSwapV2 { accounts, .. } => vec![
            (
                "twoHopSwapV2.1",
                &accounts.whirlpool_one,
                &accounts.token_vault_one_input,
                &accounts.token_vault_one_intermediate,
            ),
            (
                "twoHopSwapV2.2",
                &accounts.whirlpool_two,
                &accounts.token_vault_two_intermediate,
                &accounts.token_vault_two_output,
            ),
        ],
        _ => vec![],
    }
}

#[derive(Debug)]
struct SwapCollector;

impl vixen::Handler<whirlpool::Instructions, InstructionUpdate> for SwapCollector {
    async fn handle(
        &self,
        value: &whirlpool::Instructions,
        raw: &InstructionUpdate,
    ) -> vixen::HandlerResult<()> {
        let legs = swap_legs(&value.instruction);

        if legs.is_empty() {
            return Ok(());
        }

        let shared = &raw.shared;

        // Index token balances by the base58 pubkey of the account they belong
        // to. `account_index` points into the transaction's full account list,
        // resolved via `AccountKeys::get`. Element type is inferred from the
        // `Vec<TokenBalance>`, so we never name the gRPC proto type.
        let mut pre: HashMap<String, VaultAmt> = HashMap::new();
        let mut post: HashMap<String, VaultAmt> = HashMap::new();

        for (balances, map) in [
            (&shared.pre_token_balances, &mut pre),
            (&shared.post_token_balances, &mut post),
        ] {
            for tb in balances {
                let Ok(pk) = shared.accounts.get(tb.account_index) else { continue };
                let Some(uta) = tb.ui_token_amount.as_ref() else { continue };
                let Ok(amount) = uta.amount.parse::<u128>() else { continue };

                map.insert(
                    pk.to_string(),
                    VaultAmt { mint: tb.mint.clone(), decimals: uta.decimals, amount },
                );
            }
        }

        let program_id = raw.program.to_string();

        for (kind, pool, vault_a, vault_b) in legs {
            let (Some(a_pre), Some(a_post), Some(b_pre), Some(b_post)) = (
                pre.get(&vault_a.to_string()),
                post.get(&vault_a.to_string()),
                pre.get(&vault_b.to_string()),
                post.get(&vault_b.to_string()),
            ) else {
                continue;
            };

            let Some(((in_mint, in_amount, in_decimals), (out_mint, out_amount, out_decimals))) =
                leg(a_pre, a_post, b_pre, b_post)
            else {
                continue;
            };

            let record = SwapRecord {
                slot: shared.slot,
                program_id: program_id.clone(),
                pool: pool.to_string(),
                kind,
                in_mint,
                in_amount: in_amount.to_string(),
                in_decimals,
                out_mint,
                out_amount: out_amount.to_string(),
                out_decimals,
            };

            match serde_json::to_string(&record) {
                Ok(line) => println!("{line}"),
                Err(e) => tracing::error!(%e, "failed to serialize swap record"),
            }
        }

        Ok(())
    }
}

fn main() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .init();

    let Opts { config } = Opts::parse();
    let config = std::fs::read_to_string(&config)
        .unwrap_or_else(|e| panic!("Error reading config file {}: {e}", config.display()));
    let config = toml::from_str(&config).expect("Error parsing config");

    vixen::Runtime::<YellowstoneGrpcSource>::builder()
        .instruction(Pipeline::new(whirlpool::InstructionParser, [SwapCollector]))
        .build(config)
        .run();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn va(mint: &str, decimals: u32, amount: u128) -> VaultAmt {
        VaultAmt { mint: mint.to_owned(), decimals, amount }
    }

    #[test]
    fn a_to_b_direction() {
        // Pool receives A (+1000), sends B (-995).
        let (input, output) =
            leg(&va("A", 9, 10_000), &va("A", 9, 11_000), &va("B", 6, 50_000), &va("B", 6, 49_005))
                .expect("opposite-direction deltas must yield a leg");

        assert_eq!(input, ("A".to_owned(), 1_000, 9));
        assert_eq!(output, ("B".to_owned(), 995, 6));
    }

    #[test]
    fn b_to_a_direction() {
        // Pool receives B (+995), sends A (-1000).
        let (input, output) =
            leg(&va("A", 9, 11_000), &va("A", 9, 10_000), &va("B", 6, 49_005), &va("B", 6, 50_000))
                .expect("opposite-direction deltas must yield a leg");

        assert_eq!(input, ("B".to_owned(), 995, 6));
        assert_eq!(output, ("A".to_owned(), 1_000, 9));
    }

    #[test]
    fn no_movement_is_none() {
        assert!(leg(&va("A", 9, 10), &va("A", 9, 10), &va("B", 6, 10), &va("B", 6, 10)).is_none());
    }
}
