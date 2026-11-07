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
| 第一次全跑後 `status --unenforced` 被人看過 | 跑完（見下節）；failing 清單已分類，人看過 LiDAR / 簽名 的處置 |
| 第二個真實功能走完 | 未做 |

## 收尾（2026-11-07 下午）

- GitHub 設定完成：ruleset `evidence`（id 26757096：deletion、non_fast_forward）；Environment `evidence`（deployment branch 限 `main`）放 `EVIDENCE_SIGNING_KEY`；公鑰以 `ci` 進 `allowed_signers`；record job 掛 `environment: evidence`。dispatch 驗證：三個 record 帶 `ci` 簽名，`-Y verify` 通過；與操作者手推的 evidence 分支撞在一起，non-FF 重試正常。
- 操作者本機跑 LiDAR 四個 target（`SLIME_LIDAR_BAG` 指向 `~/Downloads/.../lidar_ros2.mcap`，SHA 吻合）：全部通過，digest 與 CI 一致；以 `iceice666` 簽名 publish。**300/406 enforced**。
- `link_wake_check` 本機也失敗，但原因不同：`zenoh-external` arm「0 sent batches reported, expected 4」，boot 前 13 秒即拒，重跑一致。真 failing，不是環境。

- tarball 已進 history 倉庫 commit `245ef0ca69d924a085756647c0a15bc20baaabca`，`docs/history.md` 已回填。

## 待辦（Phase 3 結束時）

| # | 事項 | 誰 | 規則 / 去處 |
| --- | --- | --- | --- |
| 1 | `system_test_run_inventory_check` rot（連帶 `system_image_closure_gates_check`，8 條 requirement，`contracts` area）：arm 數量 control 過時 | 下一個碰 `contracts` area 的 PR | E3：修或標 planned |
| 2 | `zenoh_service_grants_reader_check` rot（5 條，`hardware` area）：checker pin 365 vs `quality.just` 369 | 下一個碰 `hardware` area 的 PR | E3 |
| 3 | `link_wake_check` failing（5 條，`planes-mechanism` area）：`zenoh-external` arm 0 sent batches；CI 另缺 linux aarch64 wheel pin | 下一個碰 `planes-mechanism` area 的 PR | E3；wheel pin 補上後可從 `operator` 移除 |
| 4 | `rpi5_ros2_zenoh_check` 3 條 claimed | 操作者在板子上 `record --signer` | D9 |
| 5 | 84 條 planned（72 在 `backlog.zt`）：open item 歸戶、無 exam | 人核哪些要做 | 計畫 Phase 3「逐一判斷」未做，順延 |
| 6 | `hardware.zt` 232 條，超過 D11 reopen 訊號（60） | 人核 | 按 `docs/architecture/` 粒度拆 |
| 7 | 「第二個真實功能走完」離場條件 | 下一個真實功能 | 未做 |
| 8 | 歸戶的 rationale 是 item 標題、`problem` 在註解裡；各 area `owns` 是初稿 | Phase 3 人核 | 計畫原定；rule 4 `--enforce-owns` 等 owns 校準後開 |

## 待辦 1、2、5、6、8 的處理（2026-11-07 晚，人委託，決策見 `decisions.md` F1–F6）

| # | 做了什麼 |
| --- | --- |
| 1 | `check-system-test-run.py`：arm 數量 control 5→6（`sel4-zenoh-service` #935 加入時沒更新）；planned-arm 的 5 個 control 原本假設 `sel4-io-tcp-buffers` 還是 planned，它在 #893 落地了，改成在 `build/` 下 staged 副本裡藏掉它的 closure 與 record 來模擬。本機 `just system_test_run_inventory_check` 通過（206 個負面案例） |
| 2 | `zenoh_grants_reader_exam.py` `PINNED_TESTS` 365→369，硬編碼的 365 改用常數。本機不能跑（要 `qemu-aarch64`），等 CI。這個檔在所有 zenoh target 的 grader 集合裡（`check-zenoh-profile0.py` import 它），所以 26 個 zenoh target 會重跑，約 10 分鐘；加上 `system_image_closure_gates_check` 45 分鐘 |
| 3 | `link_wake_check` 5 條標 planned + note（E3） |
| 5 | 刪 7 條（2 個 CI 快取 item）；搬 6 組；`backlog.zt` → `network-coverage.zt`（37 條 smoltcp epic）；76 條 planned 的 `note` 改成「等什麼」 |
| 6 | `hardware.zt` → `zenoh-transport`（69）、`zenoh-service-core`（61）、`zenoh-service-client`（47）、`zenoh-service-grants`（53）、`lidar-demo`（15）。nonGoals 透過 `67b66e61` 的 `.tasks/items` 回溯到 item 再分配。只搬區塊：`status --json` 前後比對，只有 scripts 改動連帶的 target 從 enforced 變 claimed |
| 8 | `owns`：1713 個產品檔中 405 個無人擁有 → 0；CI `check --base` 加 `--enforce-owns`；`check` rule 1 把「刪掉一個 invariants 為空的 area」誤判為 invariants 改動，修正。rationale 不重寫（F5） |

仍開著：4（rpi5 板子）、7（第二個真實功能）、`link_wake_check` 的真實缺陷、Phase 4 的基線對照。
