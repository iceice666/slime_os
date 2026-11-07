# Phase 1 紀錄

2026-11-07。依 `plan.md` Phase 1 與 `decisions.md` 修正後的離場條件。

## 落地的東西

| 檔案 | 內容 |
| --- | --- |
| `contracts/observation/v1/schema.zt` | `Record`（機器）與 `HumanRecord`（人）型別；`counts` / `flags` 取代舊的 typed observations |
| `specs/schema.zt` | `Area` / `Requirement` / `Acceptance` / `Predicate` / `Decision`，含 `nonGoals`、`constrainedBy`、`depends`、`supersedes`、`planned`+`note` |
| `specs/helpers.zt` | `area`、`req`、`planned`、`observed`、`observedWhere`、`count`、`atLeast`、`atMost`、`flag`、`human`、`invariant`、`byHuman`、`byAgentDefault` |
| `specs/runtime.zt` | 第一個 area：`planes-runtime` 的 1 個 active + 2 個 done item，13 條 requirement，由 `migrate` 機械產出 |
| `scripts/spec/lib.py` | Zutai 讀取、requirementsDigest、`Recipes`、closure 解析（E1 的 digest / grader 集合）、`# spec:` 標記 |
| `scripts/spec/check` | 9 類拒絕；`--digest` |
| `scripts/spec/closure` | 一個或全部綁定 target 的 closureDigest 與組成 |
| `scripts/spec/record` | 包一個 target，寫 `build/observations/<target>.zt`，exit 透傳 |
| `scripts/spec/migrate` | dev-spec/v1 item → area 檔 |
| `just spec_check`、`just spec_closure` | |
| `check-work-items.py` `CODE_IDENTITY_EXEMPT` | 加 `proposal/`、`specs/`、`scripts/spec/` |

## 離場條件

| 條件 | 結果 |
| --- | --- |
| `just spec_check` 綠 | 1 area、13 requirement、1941 個檔掃描 `# spec:`；9 個負面案例全部拒絕 |
| 同輸入 digest 穩定 | `check --digest` 與 `closure` 各跑兩次 byte-identical |
| 一個 arm64 QEMU plane 三段 closureDigest 算得出 | `clock_authority_check`：2 recipe + 27 grader 檔 + 4 toolchain 檔 + image `sel4-clock-authority` derivation id；`clock_direct_read_check` 9 recipe + 7 image |
| wall time | 冷啟（component spec 未 compile）3 個 target / 11 個 image：2 分 3 秒；暖機單 target 1.8–4.9 秒 |
| `just tasks_check` 綠 | 447 items，104 validated；2 分 7 秒 |
| `just docs_check` 綠 | |
| ruff / typos | 綠（`scripts/spec/*` 無副檔名，`just ruff` 的 include 不含它們，手動跑過） |

## 做的時候發現的事（修正 `decisions.md`）

- **predicate 實際集合比評估寫的小**：104 個 body 裡只有 `equal/bool`（120）、`equal/int`（80）、`all`（51）、`bounds/int`（14）。`any` 與 `equals` 是 grep 到 prose。helper 因此是 `flag / count / atLeast / atMost`，list 即 all；**不需要 `any`**。評估第 6 點與 decisions「helper 集合」一列據此修正。
- **`contracts/observation/` 無法 exempt**：`codePaths` 以 `contracts` 為 prefix 覆蓋整個目錄，所以它本來就在舊 code identity 內；新增它就 stale 所有舊 evidence。D3 已接受這個代價（舊 loop 的 active item 本來就會不斷 re-record）。
- **第 (2) 段不需要 `--image`**：49 個 plane checker 在模組頂層宣告 `CLOSURE = "<name>"`，`closure_image.closure_identity` 不用 build 就能算 derivation id。`lib.image_closures` 直接掃 grader 檔裡的這個 token。沒有宣告 image 也沒有 `[metadata("inputs")]` 的 target 落到 coarse fallback（4 個 product root 的 tree id）。
- **`[metadata("inputs", ...)]` 在 just 1.56 進 `--dump`**，可以用。
- **Zutai 的 import 不能往上穿出 entry file 的目錄**：record 把 `contracts/observation/v1/schema.zt` 複製到 `build/observations/` 旁邊再 type-check（v2 同樣做法）。
- **`devloop_observations.py` 不用改**：`record` 設同一個 `DEVLOOP_JUST_TARGET`，舊 JSON report 是中間產物，兩格式並存零改動；而且它在 codePaths 裡，改了會 stale 一切。
- **`migrate` 每次執行產新 UUID**：commit 後不可重跑覆蓋既有 area 檔；Phase 3 要加 `--merge` 只補沒歸戶的 item。
- 歸戶的 rationale 用 item 標題，`problem` 以註解保留在 legacy 區塊上方，Phase 3 人核時改寫。

## 沒做 / 移到 Phase 2

- `status`、`human`、`attest`、`publish`、`check --base`（invariants 改動、owns 覆蓋、grader independence）。
- `specs/legacy.zt`：目前 `check` 以 `.tasks/items/<uuid>.md` 當 legacy 索引。
- `record` 尚未包進任何 recipe；Phase 2 的 `evidence.yml` 決定包法。
- 沒跑任何 QEMU plane；closure 可算 ≠ exam 跑過。
