# orca-swaps

Collect **every** Orca Whirlpool swap from a live Yellowstone gRPC stream and
print one JSON line per swap leg.

Program: `whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc`
Variants captured: `swap`, `swapV2`, `twoHopSwap`, `twoHopSwapV2`
(two-hop swaps emit one line per pool).

## Use it from a fresh clone

**Prerequisites**
- Rust via [rustup](https://rustup.rs) — the pinned toolchain (`rust-toolchain.toml`,
  1.90) installs automatically on first build. A C toolchain (`cc`) is needed
  for `aws-lc-rs`; it's present on most systems.
- A Yellowstone gRPC (Dragon's Mouth) endpoint + token — Triton/rpcpool, Helius,
  QuickNode, or self-hosted. Not a plain JSON-RPC URL.

**Steps**

```bash
git clone https://github.com/czaacza/yellowstone-vixen.git
cd yellowstone-vixen
git checkout feat/orca-swaps-example        # until it's merged to main

cp examples/orca-swaps/Vixen.example.toml examples/orca-swaps/Vixen.toml
$EDITOR examples/orca-swaps/Vixen.toml       # set endpoint + x-token

RUST_LOG=info cargo run -p orca-swaps        # first build pulls the Solana stack — takes a few min
```

`cargo run` reads `examples/orca-swaps/Vixen.toml` by default (override with
`--config <path>`). Save the stream to a file with `> swaps.jsonl` (JSON goes to
stdout, logs to stderr).

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
