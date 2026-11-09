# devloop V1 框架重設計提案

狀態：已決策，2026-11-10，見 [`docs/decisions/work-item-devloop.md`](../../docs/decisions/work-item-devloop.md)。實作中。

範圍：保留 V1 devloop 的四個階段 spec → impl → evidence → closeout 與
work-item 模型，重新設計每個階段裡「誰做、機器擋什麼、人看什麼」。
目標是同時滿足四件事：多個 track 平行開發、agent 做大部分事情（含決策）、
機器驗證大部分狀態（含 DbC）、人只決定方向與偏好。

本文的 V1 指 2026-11-07 前的 work-item devloop（`.tasks/`、`.devloop/`、
MyQue、devloop、closeout bot）。living-spec 指 11-07 起取代它的模型
（[決策紀錄](../../docs/decisions/living-spec-devloop.md)）。v2 指
`../slime_os-v2` 的實驗 repo。

---

## 1. 為什麼重做，而不是沿用 living-spec

living-spec 已經修掉 V1 最大的成本，但它和 V1 共用一個不成立的前提：
**人會寫 invariant、擁有契約子句、核准 grader 改動。** 實測不是這樣。

### 1.1 V1 的成本結構（來源：`living-spec-devloop.md`、v2 `docs/v1-portability.md`）

| 指標 | 實測 |
| --- | ---: |
| 最後 200 個 commit 中的 `repo/tasks` bookkeeping | 123 |
| 全部 318 個 commit 中碰產品碼的 | 52（16%） |
| 全部 318 個 commit 中碰 `.tasks/` 的 | 233 |
| 一個 feature 的平均 PR 數 | 4 |
| evidence digest 綁定的路徑數 | 312，加 24 小時 TTL |
| 每個 feature 的 evidence 重錄次數 | 約 5 |
| 修一個 grader | 要另開一個 work item |

### 1.2 人的實際參與（來源：`../slime_os-v2` 的 33 個 pi session）

| 指標 | 實測 |
| --- | ---: |
| 人類訊息總數 | 163 |
| 訊息長度中位數 | 12 字 |
| 20 字以內的訊息 | 108 |
| 最常見訊息 | `commit`（15 次）、「目前進度如何」（約 12 次）、`開始`、`繼續`、`A`、「照建議」 |

有實質內容的訊息屬於五類：方向（「Phase 3 用 RPi 5B」）、經驗教訓
（「不想重演 v1 分段擴充私有記憶體上限的故事」）、直覺風險（「對 spike
夠用，但之後會卡瓶頸」）、對流程的不滿（「DbC 不夠強制，感覺是 LLM 在
enforce」）、實體觀察（「replug 了」、「板子 all pass」）。沒有一條是形式化的
invariant。invariant、契約子句、`nonGoals` 全部由 agent 寫，人回答「都同意」。

### 1.3 v2 的教訓

v2 的 devloop 設計上有 evidence branch、PR gate、`HUMAN_OWNERS`，但它一直
local-only。結果是 29 個 spec 全部 open，126 條 acceptance 停在 `claimed`，
0 條 `enforced`；近 7 天產品碼 commit 佔 40%，低於它自己定的 60%。
教訓有兩條：**gate 沒有執行環境就只剩形式**；**把判斷權交給人、人卻回答
「你選」，等於沒有錨點。**

### 1.4 結論

四個階段和 work-item 的形狀是對的。要換掉的是三個機制：

1. 人核准 → 機器 ratchet；
2. evidence 在樹裡、綁 TTL → evidence 在 branch、綁 closure digest；
3. 多個 PR 流程 → 一個 PR 帶 item、impl、exam、closeout。

---

## 2. 不變的部分

| 保留 | 理由 |
| --- | --- |
| work-item 是一個檔、一個 UUIDv7、有 parent/depends | 身分穩定、離線、可查 |
| problem / scope / nonGoals / requirements / acceptance / predicate 這組概念 | 形狀已驗證可用；見 `.devloop/examples/stale-handle-rejection.zti` |
| `.devloop/policy.json` 的 gate 表 | requirement 不帶命令，gate 解析到 `just` target 的設計正確 |
| typed observation（`casesObserved`、`negativeControlsRefused`…） | 比 exit code 強，是 DbC 的前身 |
| 五條 devloop 不變式 | done 由機器算；受測碼不能改 grader；evidence 綁 closure；人決定 scope 與 known limit；離線、UUID |

第四條不變式改寫為：**人決定方向、具體例子的對錯、以及 ratchet 退步是否
接受。** 不再要求人寫 invariant。

不保留的：Markdown frontmatter + fenced `.zti` 的 body 形式。理由見 §3.1。

---

## 3. 四個階段的設計

### 3.1 work-item 的格式：`.zt` 取代 fenced `.zti`

V1 的 body 是資料形式的 `.zti`，每個欄位都要寫滿。一條 predicate 要 15 行：

```text
{
  id = "four-stale-attempts-refused";
  kind = "equal";
  observation = "staleHandlesRejected";
  expected = { kind = "int"; intValue = 4; boolValue = false; textValue = ""; };
  lower = 0; upper = 0; lowerInclusive = false; upperInclusive = false;
  members = [];
};
```

v2 改成 `.zt` 原始碼，透過 `helpers.zt` 的 constructor 寫，同一條是一行：

```text
h.count "staleHandlesRejected" 4
```

本提案採 v2 的形式。一個 work-item 是 `.tasks/items/<slug>.zt`，最後一個
表達式是 `s.Spec`。`schema.zt` 與 `helpers.zt` 從 v2 搬來，加上 §3.2 的新
欄位。`.zti` 與 `requirementsDigest` 由 `tasks_check` 計算，不 commit。

| 性質 | V1 fenced `.zti` | 本提案 `.zt` |
| --- | --- | --- |
| 每條 predicate | 約 15 行 | 1 行 |
| 預設值 | 每個欄位都要寫 | constructor 補 |
| 註解 | Markdown 段落，與資料分離 | `--|` doc comment 緊貼欄位 |
| digest | 算文字 | 算 evaluate 後的值；改註解或排版不使 evidence 失效 |
| 檢查 | MyQue + devloop 外部二進位，每次編譯連結 | `zutai` 一個二進位，`scripts/spec/` ≤ 1500 行 |
| 工具相依 | MyQue、devloop、myque-gh 三個 pin | 只有 `deps/zutai` |

`stale-handle-rejection` 在本提案的形式：

```text
s ::= import "../schema.zt";
h ::= import "../helpers.zt";

spec :: s.Spec = (h.task "01a1xxxx-..." "stale-handle-rejection"
  "Stale handles are refused after a restart"
  "After a service restarts under a fresh epoch, a client holding a predecessor handle must be refused on every operation. Today the plane gate asserts only that the restart marker chain completes.")
  with {
    owns = { "slime-root/src/task.rs"; "slime-root/src/supervision.rs"; };
    nonGoals = "No transparent reconnection; no physical-machine evidence.";
    requirements = {
      h.req "stale-refused"
        "Every operation with a predecessor-epoch handle is refused and counted as one marker per attempt."
        { "lifecycle_restart_check"; };
      h.req "controls-hold"
        "A missing marker, a reordered chain and an explicit failure marker each fail the gate."
        { "lifecycle_restart_check"; };
    };
    acceptance = {
      h.observedWhere "lifecycle_restart_check" {
        h.count "staleHandlesRejected" 4;
        h.atLeast "negativeControlsRefused" 3;
      };
    };
    invariants = {
      h.invariant "refused-no-state" "A refused input changes no state."
        (h.fromHuman "回收的空間要等整個 parent 被回收才會釋出...之後應該會卡瓶頸" "pi session 2026-11-05T14-52");
    };
  };

spec
```

預設值由 constructor 補；只有 milestone 必須寫滿 owns / nonGoals /
requirements / acceptance / invariants / decisions。

---

### 3.2 spec

**一個 PR。** item 檔和實作在同一個 PR。取消 planning-first 與獨立的
admission PR；admission 變成 CI 檢查 `tasks_check`。

**`provenance` 欄位。** 每條 requirement、nonGoal、invariant 帶一個出處，
用三個 constructor 寫：

```text
h.fromHuman    "因為我不想重演 v1 分段擴充私有記憶體上限的故事" "pi session 2026-11-08T07-02"
h.fromDecision "docs/decisions/memory-capacity.md"
h.agentDefault
```

`h.invariant` 與 `h.req` 多一個 provenance 參數；省略時等於 `h.agentDefault`。
`agentDefault` 不擋 merge。`tasks_status` 印出每個 item 的 `agentDefault`
佔比。這是衡量「有多少規則其實沒人要」的唯一數字。

**`owns` 欄位。** item 宣告它擁有的產品路徑（crate 或目錄）。CI 檢查 PR
碰到的產品路徑落在它 item 的 `owns` 內。兩個 open item 的 `owns` 重疊時，
後開的進不了 open。

**新 acceptance 類型 `h.clause`。** 除了 `h.observed`、`h.observedWhere`、
`h.contract`、`h.human`，加 `h.clause <interface> <contractId> <clauseId> <backends>`，
指向 `contracts/**` 裡的一條 pre / post / invariant 子句。verdict 由 §3.4 的
oracle 產生，不由 exam 自報。v2 的 `h.contract` 以整個 contract 為單位，
`h.clause` 細到子句，是 ratchet `R2` 的計數單位。

**新欄位 `knownLimits`。** `h.knownLimit <ratchetId> <reason> <provenance>`。
§3.4 的 ratchet 退步需要它。

### 3.3 impl

**三個 agent 角色，不同 context。**

| 角色 | 可寫 | 不可寫 |
| --- | --- | --- |
| spec writer | `.tasks/items/`、`contracts/**` 子句、oracle、fixture | `components/`、`slime-root/`、`scripts/check/` |
| implementer | `components/`、`slime-root/`、exam 的 recipe | `contracts/**` 子句、oracle、negative control |
| breaker | negative control、mutation control、bad trace | 產品碼、契約子句 |

角色由 commit trailer `Devloop-Role:` 宣告。CI 對照每個 commit 的 diff 路徑
與角色允許集合，不符則拒絕。同一個 PR 可含三種角色的 commit。

這取代 V1 的「人核准 grader 改動」：擋的對象從「人沒看過」變成「同一個
context 不能既出題又作答」。它不防兩個 agent 串通；§6 列為 known limit。

**決策委派規則。** agent 遇到選擇時，依可逆性分流：

| 類型 | 處理 |
| --- | --- |
| 可逆，且在 item 的 invariants 內 | agent 決定，寫入 item 的 `decisions`，`provenance.kind = "agentDefault"` |
| 碰到 `contracts/**` 既有子句、`owns` 範圍、不可逆的持久化格式 | 停下，用一個具體例子問人（§4） |

### 3.4 evidence

**evidence 不進樹。** record 寫到 orphan branch `evidence`，由 `push: main`
觸發的 workflow 寫入。agent 帳號不能 push 該 branch。本地 record 進
`target/observations/`（gitignored），`tasks_status --local` 可讀。
這一條直接沿用 living-spec 的實作。

**digest 只算真正輸入。** 一個 target 的 closure digest =
recipe 定義 + 該 target 的 grader set + 它開機的 image derivation id +
toolchain pin。不含整個 `scripts/`、不含 TTL。

**DbC oracle 為第五種 gate。** 搬 v2 ADR 0002 的 typed trace + pure Zutai
oracle：

```text
backend (QEMU / board / host-sim)
   │ emits raw typed Event, no verdict
   ▼
Trace { schema; events }
   │
   ▼
Zutai oracle: Trace -> List Violation     ← grader, spec writer 擁有
   │
   ▼
exam: good trace → 0 violations
      bad trace → expected clause
      mutation control → implementation breaks → violation
      dropped-event control → adapter 漏事件 → violation
```

規則：backend 不產 `passed`；fixture 的 expectation 與 backend 的 observation
分開；oracle 只讀 trace。一條子句要算「被覆蓋」，必須同時有 positive case
與至少一個會失敗的 negative control。

**Ratchet。** 每次 PR 對 base 計算四個數字：

| id | 定義 | 允許方向 |
| --- | --- | --- |
| `R1 clauseCount` | 每個 interface 的契約子句數 | 不減 |
| `R2 clauseCoverage` | 有 oracle check 且有會失敗的 negative control 的子句 ÷ 全部子句 | 不降 |
| `R3 mutantSurvival` | `cargo mutants` 在 `owns` 內 crate 的存活率 | 不升 |
| `R4 negativeControls` | 每個 target 的 `negativeControlsRefused` 計數 | 不減 |

任一項退步，CI 擋下。放行條件只有一個：PR 的 item 新增一條 `knownLimits`
指明退步的 `R` id 與理由，且 PR 有 `HUMAN_OWNERS` 的 approval。
**這是整個流程唯一保留人核准的地方。** 問題對非專家可答：「這次檢查的東西
變少了，接受嗎？」

變好或不變，不需要任何人核准。

### 3.5 closeout

**state 是計算值。** item 的 frontmatter 不再寫 `state`。`tasks_status`
從 evidence branch 算出每條 acceptance 的狀態與 item 的總狀態：

| acceptance | 條件 |
| --- | --- |
| `enforced` | HEAD closure digest 有通過的 record |
| `failing` | HEAD closure digest 有失敗的 record |
| `claimed` | 無 record |
| `advisory` | `mandatory = false` |

item 為 `done` 當且僅當每條 mandatory acceptance 為 `enforced`。

**關閉 = 移檔。** 把 item 從 `.tasks/items/` 移到 `.tasks/terminal/`，
作為下一個碰該 `owns` 的 PR 的一部分，或獨立的 chore commit。CI 檢查移動時
`tasks_status` 對該 item 為 `done`。取消 closeout bot、retirement workflow
與它們的 checker。

**failing 的處理。** main 上 `failing` 的 acceptance 由下一個碰同一 `owns`
的 PR 修掉，或在 item 加 `knownLimits`（走 §3.4 的 approval）。CI 擋「碰到該
`owns` 但兩者都沒做」的 PR。

---

## 4. 人的角色

流程只在四個時機找人，每次問題可用一句話回答：

| 時機 | 問法 | 預期回答 |
| --- | --- | --- |
| 開 track | 「下一個 milestone 做 X 還是 Y？各自的代價是…」 | 一個字母 |
| 具體例子 | 「spawn 收到第 5 個參數時，拒絕整個請求，還是只丟掉第 5 個？」 | 一句話 |
| ratchet 退步 | 「`R1` 從 12 降到 10，agent 說因為 Z。接受成 known limit 嗎？」 | yes / no |
| 實體觀察 | 「板子接好了嗎？」 | `replug了` |

agent 不問「這條 invariant 寫得對嗎」。agent 把 invariant 翻成具體的輸入與
預期結果再問。人對例子的回答進 `provenance.quote`。

取消 `HUMAN_OWNERS` 對 grader set 改動的 approval（§3.4 的 ratchet 退步
除外）。理由：人核准一份看不懂的 checker diff，給機器的訊號是「有人看過」，
實際沒有。假核准比無核准更糟。

人的每週例行只有一項：看 `tasks_status --weekly` 的四個數字（§7）與
`agentDefault` 清單，翻掉不想要的 default。

---

## 5. 平行 track

| 機制 | 規則 |
| --- | --- |
| worktree | 每個 track 一個 worktree、一個 item 子樹（parent + children） |
| `owns` 互斥 | 兩個 open track 的 `owns` 不得重疊；CI 在 item 進 open 時檢查 |
| 共用程式庫 | `components/lib`、`scripts/lib` 不可被 `owns`；改它們的 PR 必須通過所有 track 的 gate |
| 衍生檔 | closure sources、generated bindings 不在 PR 內重產；merge 後 workflow 產並 commit |
| 上限 | 同時 open 的 track ≤ 3；超過則 `tasks_check` 擋 |
| QEMU 時間 | evidence workflow 依觀測 wall time 分 shard（living-spec 已有） |

衍生檔搬到 merge 後重產，是消除平行 PR 衝突的主要手段。代價是 PR 內的
closure check 要改為「對 base 的衍生檔檢查」而非「對 HEAD」。

---

## 6. 風險與 known limits

| 風險 | 處理 |
| --- | --- |
| agent 寫恆真子句灌 `R1` | `R2` 要求每條子句有會失敗的 negative control；恆真子句無法有，`R2` 會降 |
| 兩個 agent 角色串通 | 不防。角色分離是 context 分離，不是權限分離。要防需獨立帳號與 branch protection，列為 reopen 條件 |
| `provenance.kind = "human"` 被 agent 偽填 | 不驗簽。quote 可對照 session 紀錄事後查核；偽填是流程違規，不是技術漏洞 |
| ratchet 只擋退步，不擋原地踏步 | 接受。原地踏步由 §7 的指標暴露 |
| oracle 只判收到的 event | adapter 漏事件由 dropped-event control 擋；外部事實（UART、disk）仍需獨立 probe |
| 有限 trace 不證明 liveness 與全部輸入 | 接受。Kani 與形式化證明是後續工作，不在本提案 |
| 衍生檔 merge 後重產的 commit 是 bot commit | 接受。一個 feature 最多一個，且不碰 `.tasks/` |

---

## 7. 指標

| 指標 | V1 實測 | v2 實測 | 目標 |
| --- | ---: | ---: | ---: |
| 碰產品碼的 commit 比例 | 16% | 40% | ≥ 60% |
| 每個 feature 的 PR 數 | 4 | 不適用 | 1 |
| 每個 feature 需要人回答的次數 | 未量 | 未量 | ≤ 2 |
| `agentDefault` 佔全部 invariant 的比例 | 未量 | 未量 | 有數字，趨勢下降 |
| `claimed` 轉 `enforced` 的中位時間 | 未量 | 永遠 claimed | ≤ 1 個 main push |
| 需要 `HUMAN_OWNERS` approval 的 PR 比例 | 未量 | 不適用 | ≤ 10% |

`tasks_status --weekly` 印這六個數字。

---

## 8. 分階段

| 階段 | 做什麼 | 完成條件 |
| --- | --- | --- |
| P0 | evidence 搬到 branch，去 TTL，digest 縮到真正輸入 | 一個既有 plane 的 record 在 `evidence` branch 上，連續兩次 main push 不重錄 |
| P1 | `tasks_status` 計算 state；取消 closeout bot 與 retirement workflow | 既有 open item 全部有計算狀態；bookkeeping commit 歸零 |
| P2 | ratchet `R3`、`R4`（不需 oracle） | 一個故意刪 negative control 的 PR 被擋 |
| P3 | oracle 試點：一個 interface、一條子句、四種 control | `R1`、`R2` 對該 interface 有數字 |
| P4 | 角色分離與 `Devloop-Role` trailer | 一個 implementer commit 碰 oracle 被擋 |
| P5 | `owns`、衍生檔 merge 後重產、track 上限 | 兩個 track 平行合入，無手動 conflict |

P3 的候選 interface：spawn 的 argument framing。`specs/planes-mechanism.zt`
已有可形式化的句子（「refused before any child is spawned, and the refusal
releases every capability the client handed over」），且已有 negative control。

P0 到 P2 不碰產品碼，可在 living-spec 現況上直接做。P3 起才需要 v2 的
oracle 搬移。

---

## 9. 待決策

格式沿用 `proposal/devloop-redesign/decisions.md`：
`Decision: <option>. Because <one sentence>. Reopen if <signal>.`
未決時由 agent 以 default 推進，標 `byAgentDefault`。

| # | 問題 | 選項 | agent default |
| --- | --- | --- | --- |
| Q1 | 本提案在哪個 repo 試行 | (a) `slime_os` 直接改 living-spec；(b) `slime_os-v2` 先試再搬 | (a)。v2 沒有 CI，§1.3 已證明 gate 無環境就只剩形式 |
| Q2 | `R3 mutantSurvival` 的量測範圍 | (a) 全 workspace；(b) 只 `owns` 內 crate | (b)。全 workspace 一次要數小時 |
| Q3 | ratchet 退步的 approval 來源 | (a) `HUMAN_OWNERS` PR approval；(b) item 內簽名 record | (a)。已有實作，對非專家可答 |
| Q4 | 角色分離是否要獨立帳號 | (a) 只用 trailer；(b) 三個 bot 帳號 + branch protection | (a)。先量串通是否真的發生 |
| Q5 | living-spec 的 `specs/<area>.zt` 與 work-item 的關係 | (a) area 檔退為 item 的索引與 invariants 容器；(b) 廢除 area 檔 | (a)。area 的 `invariants`、`openQuestions` 沒有別的家 |
| Q6 | 衍生檔 merge 後重產的失敗處理 | (a) revert 該 merge；(b) 開 failing 標記等下一個 PR | (b)。revert 會打斷平行 track |
| Q7 | work-item 檔名 | (a) `<slug>.zt`，UUID 在檔內（v2）；(b) `<UUIDv7>.zt`（V1） | (a)。可讀；`tasks_check` 擋 UUID 重複與 slug 改名後的 parent/depends 斷鏈 |
| Q8 | `schema.zt` / `helpers.zt` 的來源 | (a) 從 v2 複製進 `.tasks/`，之後獨立演進；(b) 抽成獨立 repo 由兩邊 pin | (a)。v2 是實驗 repo，不會長期維護；獨立 repo 是 V1 三個外部 pin 的老路 |

---

## 10. 未驗證

本提案未執行任何 runtime test。§1 的數字來自 `git log`、
`docs/decisions/living-spec-devloop.md`、v2 `docs/v1-portability.md`、
v2 的 `just spec-status` 輸出與 pi session 紀錄的訊息統計。§7 的「未量」
表示 V1 與 v2 都沒有留下可計算的紀錄。
