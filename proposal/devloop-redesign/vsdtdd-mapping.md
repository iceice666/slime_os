# vsdtdd 模型 ↔ devloop 重設計 對照

來源：[`vsdtdd-model.png`](vsdtdd-model.png)（2026-11-07 提供）。目的：計畫 Phase 1 要「跟 vsdtdd 的作者對 `specs/*.zt`、evidence branch 與 `# spec:` 標記的格式」，這份就是要對的內容。

## 結論

兩邊的三層結構與四態一一對應，不需要改模型；要對齊的只有五件事：狀態名、反向標記格式、`depends` 邊、ADR 邊、reader 讀什麼。

## 結構對照

| vsdtdd | 本計畫 | 備註 |
| --- | --- | --- |
| feature `specs/<feature>/` | area `specs/<area>.zt` | 一檔 vs 一目錄；reader 讀 `.zt` 不讀目錄 |
| behavior `specs/<feature>/<behavior>.md` | requirement（UUIDv7） | vsdtdd 的 behavior id 就是 requirement id |
| criterion `slug-N` | acceptance（`h.observed` / `h.human` 的 id） | 狀態掛在這層，兩邊相同 |
| behavior `depends` behavior | 計畫刪掉了 | 見決策 V1 |
| ADR `docs/decisions/NNNN-*.md`，`supersedes` | `docs/decisions/`（已存在，無編號）+ area 內 `decisions` | 見決策 V2 |
| ADR `constrains` spec（`spec: constrained-by`） | 無 | 見決策 V2 |
| Test `verifies` spec（`// spec: id/slug`） | `# spec: <uuid>/<acceptance>` | 同一格式：`<requirement uuid>/<acceptance id>` |
| Test「result + revision」 | evidence branch `<target>/<closureDigest>.zt` | reader 不該自己算 digest，見決策 V3 |
| Code `implements` spec（derived） | area `owns` glob | reader 用 `owns` 推 implements 邊 |
| Task packet `tasks/<date>-<topic>/`（task / brief / tests-red / review） | 無；PR 就是 task packet | 圖註「Tasks have no layer yet」，viewer 不受影響。`introduces / modifies / retires / implements / renames` 可由 PR 內 `specs/*.zt` 的 diff 推出 |
| Spirit `spirit/<date>-<topic>.md`（proposed） | `roadmap/` + area `openQuestions` | 無動作 |

## 狀態對照

| vsdtdd | 定義 | 本計畫 |
| --- | --- | --- |
| enforced | fresh passing result | enforced |
| claimed | bound, no fresh pass | claimed |
| advisory | no test bound | **planned**（綁了 target 但 `planned: true`）、以及 `h.human` 尚未觀測 |
| failing | fresh failing result | failing |

決策（已在 `decisions.md`）：不加 `mandatory`，所以計畫原本「`mandatory = false` 且無 record」那種 advisory 不存在。`spec status --json` 輸出時直接用 vsdtdd 的四個名字，`planned` 對外報成 `advisory` 並附 `reason: planned | human-unobserved`。

## 新增決策（byAgentDefault, delegated by human 2026-11-07）

| # | 決策 | 理由 | Reopen if |
| --- | --- | --- | --- |
| V1 | requirement 加 **選填** `depends: [requirement id]`，只給 viewer 與排序用；不進 `requirementsDigest`、不影響四態；`spec check` 只驗指得到 | vsdtdd 畫這條邊；成本是一個 list 欄位；E3 的 backlog 規則不靠它 | 歸戶後沒有任何 requirement 填它 |
| V2 | area 加 **選填** `constrainedBy: [docs/decisions/<file>]`；`docs/decisions/` 維持現行無編號檔名，不改成 `NNNN-`；reader 以路徑當 ADR id | AGENTS.md 已規定跨模組決定進 `docs/decisions/`；area 內 `decisions` 只放局部的 | vsdtdd reader 硬要求 `NNNN-` 前綴 |
| V3 | vsdtdd reader 的唯一輸入是 `specs/*.zt`（結構）與 **`spec status --json`**（狀態）；reader 不讀 evidence branch、不算 closureDigest | digest 演算法只能有一份；evidence branch 的路徑是 CLI 的內部格式 | reader 需要離線、無 CLI 的環境 |

## 要跟 vsdtdd 作者對的清單

1. `# spec: <uuid>/<acceptance>` 標記：Python 用 `#`，Rust 用 `//`，justfile 用 `#`；一行一個。
2. `spec status --json` 的 schema：`{ areas: [{ id, title, requirements: [{ id, statement, depends, acceptances: [{ id, target, state, reason?, record? }] }] }] }`。Phase 1 定下後用 Zutai 寫成 `contracts/spec-status/v1/schema.zt`，reader 綁這份。
3. 既有三個 reader（`slimeos-packets`、`gates-just`、`results-slimeci`）切換時點：Phase 2 並行期雙讀，Phase 3 切。
4. `owns` glob → implements 邊：reader 直接 glob，不需要 CLI。
