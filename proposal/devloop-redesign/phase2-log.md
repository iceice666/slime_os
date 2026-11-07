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

**沒驗證**：`evidence.yml` 與 `ci.yml` 的 `spec` job 沒在 GitHub 跑過（尚未 push）。第一次 push 後要看：projection shell 有沒有 `ssh-keygen`（human 驗簽要）；arm64 runner 上 `status --claimed` 的 closure 解析時間；`GITHUB_TOKEN` 能否推 `evidence`。

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
