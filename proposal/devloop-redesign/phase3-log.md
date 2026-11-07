# Phase 3 紀錄

2026-11-07。依 `plan.md` Phase 3 與 `decisions.md`。Phase 4 的文件重寫被 `docs_check` 拉進來一起做了（刪 recipe 與 doc 後 65 個失效引用，不修不能綠）。

## 五個 commit

| Commit | 內容 |
| --- | --- |
| `c9cd8701 feat(repo/specs)` | `migrate --all`：7 個 area 檔、406 條 requirement（84 planned）、`legacy.zt` 447 列、`targets.toml` |
| `a391c0ad ci(repo/spec)` | evidence.yml 分 shard：plan job → ≤8 個 record job → publish job |
| `a0f2e37c refactor(scripts/lib)` | `work_items.py` 改讀 specs；check-docs、system-test-run generator 與 controls 跟著改 |
| `f4abcb38 chore(repo)` | 刪除清單全部：`.tasks/`、`.devloop/`、2 個 workflow、28 支 script、2 個 contract、3 個 flake input、`nix/devloop.nix`、12 個 recipe、2 個 CI job；`.#projection` → `.#spec` |
| `8d4ee92c docs(repo)` | `living-spec-devloop.md` 決策、`06-spec-lifecycle.md`、CONTRIBUTING / AGENTS / HUMAN / README / glossary / just-targets / 4 個 skill / PR template；刪 4 份舊 doc |

## 歸戶結果

| 來源 | 數量 | 去處 |
| --- | --- | --- |
| done spec-driven | 57 | 7 contracts、31 hardware、10 planes-mechanism、2 planes-runtime、1 quality；6 個只綁已刪 target 的進索引（state `retired`） |
| active | 5 | 2 backlog（CI 快取兩個 process item，target 已刪）、2 planes-mechanism、1 planes-runtime |
| open spec-driven | 24 | 21 backlog（無 target）、1 hardware、1 planes-mechanism、1 product，全部 planned |
| cancelled | 18 | 索引 |
| deferred / blocked / prose | 59 | 索引 |
| terminal | 280 | 索引 |

綁定 target 54 個，其中 `rpi5_ros2_zenoh_check` 是 operator target。全量 closure 解析本機 2 分 44 秒。

## 做的時候發現的事

- **`.tasks/terminal/` 的 280 個 retired id 仍被 docs 引用 17 處**：legacy 索引必須含 terminal，不只 167。
- **只有 human gate 的 requirement**（lidar pointCloudViewed）違反「至少一條 automated」：歸成 planned 並註明，浮出來當 backlog。
- **generator 的 PLANNED_RUNS 綁 work item**：改綁 planned requirement id；`check-system-test-run` 的 controls 改造假 area 並 patch `spec_lib.SPECS`。
- **14 個 zenoh exam 讀 `.tasks/items` 交叉比對 pin**：設計上 `.tasks` 不存在就回 `(0,0)`，pin 常數自己綁，不用改（改了會 stale evidence）。
- **`system_test_run_inventory_check` 在本機 main 上本來就紅**（network checker arm 數量），CI 從沒跑它；現在它是綁定 target，第一次全跑會記 failing——計畫說的 rot。
- **歷史倉庫連不上**（`git.justaslime.dev:2222` timeout）：tarball `~/slime_os-tasks-67b66e61c9f0.tar.gz`（SHA-256 `3ab56acc…`）等你推；`docs/history.md` 已記 frozenAt commit。
- `migrate` 一併刪除：輸入已不存在。
- `evidence/duo`、`evidence/framework-cpu-boot` 留在 tree：它們是 checker fixture（`check-duo-gate-control` 讀），不是 devloop evidence；D9 不適用。
- `slime-zutai-store-save` action 留：它是 Zutai evaluation store 快取，不是 devloop 專屬。
- `FRAMEs` 進 typos identifier 清單：歸戶的 statement 原文如此。

## 第一次全跑（run 42352634695）

8 個 shard 平行，plan 17 分、最長 shard 2 小時 25 分（`sel4_large_image_check` 單個 131 分），publish 53 檔。**283/406 enforced**。

| 狀態 | 數量 | 內容 |
| --- | --- | --- |
| enforced | 283 | |
| failing | 35 | 9 個 target，見下 |
| claimed | 3 | `rpi5_ros2_zenoh_check`（operator） |
| advisory | 85 | 84 planned + 1 human gate |

9 個 failing target 的分類：

| 類 | target | 原因 | 處置 |
| --- | --- | --- | --- |
| CI 環境 | `frame_store_check`、`lidar_publish_check`、`lidar_foxglove_bridge_check`、`lidar_foxglove_demo_check` | 需要 `SLIME_LIDAR_BAG`（操作者的 mcap） | 進 `operator` |
| CI 環境 | `link_wake_check` → `zenoh_external_check` | 沒有 linux aarch64 的 eclipse-zenoh wheel pin | 進 `operator` |
| CI 設定 | `image_attestation_check` | record job shallow checkout，要 ≥3 個祖先 | `fetch-depth: 0` |
| **rot** | `zenoh_service_grants_reader_check` | checker pin `PINNED_TESTS = 365`，`quality.just` 在 Phase 3 前就是 369 | 留給人：E3 規則 |
| **rot** | `system_test_run_inventory_check`（連帶 `system_image_closure_gates_check`） | arm 數量 control 過時，CI 從沒跑過它 | 留給人：E3 規則 |

設計缺口補上：failing record 在同一 digest 下不自動重跑（否則 rot 每次 push 都燒 45 分鐘）；`workflow_dispatch` 帶 `include-failing` 手動重跑。

### 第二次（dispatch，include-failing，run 42366778824）

`image_attestation_check` 通過（476 秒）；兩個 rot target 重跑仍失敗，不是 flaky。**289/406 enforced**。

### 當前 backlog（`status --unenforced` 的 failing / claimed）

| 來源 | requirement 數 | 處置 |
| --- | --- | --- |
| `system_test_run_inventory_check` + `system_image_closure_gates_check`（rot） | 8 | E3：下一個碰 `contracts` area 的 PR 修，或標 planned |
| `zenoh_service_grants_reader_check`（rot） | 5 | 同上，`hardware` area |
| LiDAR 四個 + `link_wake_check`（operator，第一次跑的 failing record 還在） | 16 | 操作者帶 `SLIME_LIDAR_BAG` 跑 `scripts/spec/record <t> --signer` 覆蓋 |
| `rpi5_ros2_zenoh_check`（operator） | 3 claimed | 板子上跑 |
| planned | 84 | 本來就沒 exam |

## 離場條件

| 條件 | 結果 |
| --- | --- |
| 刪除清單 Phase 3 每一列不在 tree 上 | 是（`grep -rn "\.tasks\|\.devloop\|myque"` 只剩 proposal 與 history 文字） |
| CI 綠 | 是，第二次 push（第一次：`docs_gates` job 沒有 zutai → `work_items` 改純 regex 淺讀；`--plan` stdout 又被 shellHook 污染 → 寫檔） |
| 第一次全跑後 `status --unenforced` 被人看過 | 待 Evidence run 完成（8 shard，估 1–2 小時） |
| 第二個真實功能走完 | 未做 |

## 收尾（2026-11-07 下午）

- GitHub 設定完成：ruleset `evidence`（id 26757096：deletion、non_fast_forward）；Environment `evidence`（deployment branch 限 `main`）放 `EVIDENCE_SIGNING_KEY`；公鑰以 `ci` 進 `allowed_signers`；record job 掛 `environment: evidence`。dispatch 驗證：三個 record 帶 `ci` 簽名，`-Y verify` 通過；與操作者手推的 evidence 分支撞在一起，non-FF 重試正常。
- 操作者本機跑 LiDAR 四個 target（`SLIME_LIDAR_BAG` 指向 `~/Downloads/.../lidar_ros2.mcap`，SHA 吻合）：全部通過，digest 與 CI 一致；以 `iceice666` 簽名 publish。**300/406 enforced**。
- `link_wake_check` 本機也失敗，但原因不同：`zenoh-external` arm「0 sent batches reported, expected 4」，boot 前 13 秒即拒，重跑一致。真 failing，不是環境。

## 要你做的

1. 把 `~/slime_os-tasks-67b66e61c9f0.tar.gz` 推進 history 倉庫，commit hash 回填 `docs/history.md`。
2. 三個真 failing 目標（`system_test_run_inventory_check`、`zenoh_service_grants_reader_check`、`link_wake_check`）：E3 規則，下一個碰 `contracts` / `hardware` / `planes-mechanism` area 的 PR 修或標 planned。
