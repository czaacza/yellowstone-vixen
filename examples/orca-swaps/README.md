# orca-swaps

Collect **every** Orca Whirlpool swap from a live Yellowstone gRPC stream and
print one JSON line per swap leg.

Program: `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc`
Variants captured: `swap`, `swapV2`, `twoHopSwap`, `twoHopSwapV2`
(two-hop swaps emit one line per pool).

## Run (one command)

1. Put your gRPC endpoint + x-token in `Vixen.toml` (this folder).
2. From the repo root:

```bash
RUST_LOG=info cargo run -p orca-swaps
```

Each swap prints as JSON:

```json
{"slot":312345678,"program_id":"whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc","pool":"HJPjoWUARjVGigRvfXm8kv84fPvGZm8SU4pgWfYuMSuY","kind":"swap","in_mint":"So11111111111111111111111111111111111111112","in_amount":"1000000000","in_decimals":9,"out_mint":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","out_amount":"152340000","out_decimals":6}
```

`in_amount` / `out_amount` are raw base units; divide by `10^decimals` for the
UI value. Effective price = `out_amount/10^out_decimals ÷ in_amount/10^in_decimals`.

## Offline sanity check (no endpoint needed)

```bash
cargo test -p orca-swaps
```

Verifies the in/out direction logic on synthetic vault deltas.

## How it works

- The parser is generated at compile time from `whirlpool.json` (a Codama IDL)
  via `include_vixen_parser!`. Its prefilter already scopes the gRPC
  subscription to the Orca program — you only receive Orca transactions.
- Real executed amounts and mints come from the transaction's pre/post **token
  balances** on the pool vaults, not the instruction args (which only carry the
  *specified* amount, not both sides).

## Known limits

- `ponytail:` amounts are the pool vaults' net change across the whole
  transaction. If the *same pool* is swapped twice in one transaction (rare),
  the two swaps net together. Upgrade path: attribute per-instruction via the
  swap's inner token-transfer CPIs (`InstructionUpdate::inner`).
- Failed transactions are not parsed. Add a failed-topic handler if you need them.
