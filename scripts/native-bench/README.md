# Native shell benchmark

This benchmark compares the Tabularis Tauri app with the experimental GPUI shell (`tabularis native`).
It runs on macOS only.

## What it measures

Each app runs one query on an empty SQLite file. The query returns 1,000,000 rows with 6 columns.

- `first_paint_ms`: time from the start of the run to the first painted result rows.
- `all_loaded_ms`: time until every row is in the grid. The value is `na` when the app loads only the first page.
- `scroll_p50_ms`, `scroll_p99_ms`: frame interval while the grid scrolls down 2000 px per frame (maximum 600 frames).
- `peak_rss_mb`: peak memory of the app and its helper processes.
- `rows`: number of rows in the grid.

Scenarios:

- `native`: the GPUI shell.
- `tauri-500`: the Tauri app with a page size of 500 rows.
- `tauri-1m`: the Tauri app with a page size of 1,000,000 rows.

## Prerequisites

- Rust toolchain, `pnpm`, and Node.js.
- Dependencies for both builds (`pnpm install` runs in the script).
- A clean checkout of the files that `tauri-bench.diff` changes.

## How to run

```sh
scripts/native-bench/run.sh --runs 5
```

The script prints each run, then a Markdown table with the median of each metric.
Set `BENCH_TIMEOUT_S` to change the 180 s timeout for each run.

The script uses a temporary data directory (`TABULARIS_DATA_DIR`). It does not touch your real Tabularis data.
It applies `tauri-bench.diff` to build the Tauri app. It always reverts the patch when it stops.
The patch adds a bench driver to the web UI and three bench commands to the Tauri app. Do not commit it.

A run that times out, crashes, or prints `BENCH error=` counts as a failure. The failure count is in the table.

## How memory is counted

The script samples memory (RSS) every 100 ms while the app runs. The peak is the highest sum.

- Native: the app process.
- Tauri: the app process plus every WebKit process that started after the run began.
  On macOS, WebKit helpers (`com.apple.WebKit.WebContent`, `Networking`, `GPU`) are not children of the app.

## Known limits

- The script cannot tell which app owns a WebKit helper. Do not start other WebKit apps (Safari, other Tauri apps) during a run.
- The Tauri scroll test measures `requestAnimationFrame` intervals. They show the frame cadence, but not the real GPU present time.
- The Tauri first-paint time counts two animation frames after the grid renders its first row.
- Release builds are not bundled or signed.
