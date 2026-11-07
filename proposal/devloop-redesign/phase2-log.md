# Phase 2 紀錄

2026-11-07。依 `plan.md` Phase 2 與 `decisions.md`。舊 loop（`.tasks/`、`.devloop/`、closeout bot）一個檔都沒刪。

## 落地的東西

| 檔案 | 內容 |
| --- | --- |
| `scripts/spec/status` | 四態 + planned→advisory；`--local`、`--json`（V3 reader 介面，`spec-status/v1`）、`--unenforced`、`--claimed`、`--area` |
| `scripts/spec/human` | `ssh-keygen -Y sign -n slime-spec`，綁 requirementsDigest |
| `scripts/spec/record --signer` | D9：操作者簽機器 record |
| `scripts/spec/publish` | plumbing commit 到 `refs/heads/evidence`；拒 digest 不符、dirty、human 簽名不過；`--require-signed` |
| `scripts/spec/check --base` | 規則 1 invariants、2 既有 `contracts/**/schema.zt`、3 grader independence（recipe closure 定義 + grader 檔）、4 owns 覆蓋（報告；`--enforce-owns` 才擋）；approvers ∩ `HUMAN_OWNERS` |
| `scripts/spec/e2e` | 臨時 clone + 臨時 ssh key，29 項檢查，12 秒 |
| `HUMAN_OWNERS`、`specs/allowed_signers` | 各一行：`iceice666` |
| `specs/zutai.toml` | Zutai pin（metadata）；`check` 驗 toml / gitlink / flake input 一致 |
| `ci.yml` `spec` job | `spec_check` + e2e；PR 上 `check --base` 帶 approvers |
| `evidence.yml` | push main：`status --claimed` → 逐個 `record --runner ci` → `publish` → push，non-FF 重試一次；簽名 key 接 `EVIDENCE_SIGNING_KEY` secret，尚未設 |

## 離場條件

計畫原文：「一個真實功能走完 spec diff + exam + 實作同一 PR 合進 main，`spec status` 在 main 上自動把它的 requirement 算成 enforced，過程中沒有任何 bot commit」。現在不走 PR、直接操作 main，等價條件在本機驗過：

| 步驟 | 結果 |
| --- | --- |
| `record robot_withdrawn_check` | 74 秒（controls + 1 plane + 4 boots），record @ `a76613c3ccc3` |
| `publish` | 1 檔進 `refs/heads/evidence`，commit `9f1828a2`，main 上零 commit |
| `status --area runtime` | 3/13 requirement enforced（robot withdrawn 的三條），其餘 claimed |
| e2e | 29/29 |
| `spec_check`、`docs_check`、ruff、typos | 綠 |

### GitHub 上的結果（push 後，4 輪修正）

| Run | 結果 | 學到 |
| --- | --- | --- |
| 1 | `spec` job 紅：`deps/zutai/stdlib` 不存在 | projection shell 不 checkout submodule；`lib.stdlib_root()` 改為 env → submodule → `<zutai-cli>/../share/zutai/stdlib` |
| 1 | Evidence 紅：`status --claimed` 的輸出被 shellHook 的 rustup 訊息污染 | 改為 `scripts/spec/ci` 在一次 shell 內做 claimed → record → publish |
| 2 | `spec` job 紅：e2e 的 publish 缺 git identity；`work_items` 紅：`HUMAN_OWNERS` 未 exempt | `commit-tree` 固定用 `evidence <evidence@slime-os>`；exempt 加一列 |
| 2 | Evidence：`sel4_lifetime_check` 82 秒失敗 | 不是 rot：`test_sel4_root` 要 `build/sel4-prefix`，其他 CI job 都先跑 `sel4_qemu_image_check`；workflow 補一步 |
| 3 | Evidence：push 被拒 `directory file conflict` | 遠端殘留 `evidence/data-volume-and-aggregate`、`evidence/image-attestations`（已 merge 的 #912/#921 分支）；刪除 |
| 3 | Evidence：`robot_withdrawn_check` 6 秒失敗（「restart total did not land between two chunks of the declared load」），前後兩次都 23 秒通過 | **flaky plane**，計畫預言的 rot 類型；同一 digest 下 passed 來回跳會留在 evidence branch 的 git log 裡。Phase 3 要排進 backlog |
| 4 | **Evidence 綠** | `clock_direct_read_check` 913 秒、`robot_withdrawn_check` 23 秒、`sel4_lifetime_check` 740 秒；publish 3 檔；`evidence` branch 建立；`status --unenforced` **13/13 enforced**；main 零 commit |

本機（macOS）算出的 closureDigest 與 CI（Linux arm64）完全一致，`status` 直接讀 `origin/evidence` 得 13/13。

CI 時間：slime-env + `sel4_qemu_image_check` 約 10 分；三個 target 共 28 分；全 run 約 45 分。這是 Phase 0 量測 (2) 的第一個數據點：3 個 target / 11 個 image。全部 ~85 個 plane recipe 按比例估 4–6 小時，超過單 job 180 分鐘上限；Phase 3 全量歸戶前要分 shard 或分 plane job。

## 做的時候發現的事

- **D1 修正**：flake 改 `src = ./deps/zutai` 牽動 41 個 `nix develop` 呼叫點，而 projection job 刻意不 checkout submodule。併入 Phase 5；Phase 2 以一致性檢查代替。
- **grader 集合用 base 的 recipe dump**：`Recipes.load_at(rev)` 以 `git archive` 抽 Justfile + `just/` 到暫存目錄跑 `just --dump`。規則 3 比對 base 與 head 的 recipe closure 定義（含 attributes），再比 base closure 的 grader 檔是否在 diff 裡（M/D/R/T）。新增檔自由進——e2e 有測。
- **行數預算 2000，排除 `migrate`**：CLI 本體 1565 行。
- **`publish` 的 parent**：本機沒有 `refs/heads/evidence` 時以 `origin/evidence` 為 parent，否則 CI 第二次跑會開新 root。
- **`git checkout -- <untracked-with-add -N>` 會清空檔案**：寫 e2e 時踩到，提醒：`add -N` 的檔不要用 `checkout --` 還原。

## 待人決定 / 設定（Phase 3 前）

1. GitHub：建 Environment `evidence`（deployment branch 限 `main`），放 `EVIDENCE_SIGNING_KEY`，對應公鑰以 principal `ci` 加進 `specs/allowed_signers`。
2. GitHub：`evidence` branch 的 ruleset——禁 force push / 刪除，push 限 CI token 與 `allowed_signers` 內的人。
3. push 一次看 `spec` job 與 `evidence.yml` 的真實結果。
