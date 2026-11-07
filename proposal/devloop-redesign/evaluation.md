# devloop 重設計計畫 — 評估

對象：[`plan.md`](plan.md)（PDF 2026-11-05 修訂版）。
對照：`slime_os` main @ `027ce9eb`（2026-11-06）、`slime_os-v2` @ `92915ae` 的 `docs/starter-kit/devloop.md` 與 `scripts/spec/`。
評估日期：2026-11-07。所有數字是我在 checkout 上實際算出來的；沒有跑任何 QEMU 或 CI。

## 結論

計畫方向正確，可以開工，但有 4 件事要在 Phase 1 之前改掉或補上：

1. **D8 建議 (a)「整個 product tree 一個 digest」會讓每次 main push 重跑全部 ~85 個 plane target。** v2 已經做出並用 e2e 驗過更好的方案（recipe 宣告 `inputs` + sandbox 讓漏算的輸入直接讓 exam 失敗），應該改採 v2 方案，並把 QEMU plane 的 product 段直接用既有的 system-image closure derivation id。
2. **grader independence 缺 v2 已經做出來的「digest 集合 ≠ grader 集合」區分**，否則 `Cargo.lock`、`sel4/pins.toml`、product 路徑每改一次都觸發 approval，而且 recipe 本身必須明確列入 grader 集合，否則「新增 checker 檔自由進」有洞。
3. **刪掉 `depends`/`parent` 之後沒有東西取代 backlog-first 規則**，計畫要說 `failing` 在 main 上代表什麼（擋誰）。
4. **CI 簽 record 的 key 必須只在 `push: main` 的 protected environment 可見**，否則有 write 權的 agent 可以用 PR 裡改過的 workflow 偽造簽名 record。

另有幾處文件層次的不一致與數字誤差，列在最後。

## 計畫保住了哪些 invariant

| # | Invariant | 評估 | 依據 |
| --- | --- | --- | --- |
| 1 | done 由機器判定 | 保住，而且比今天強：沒有 `done` 狀態，狀態純算 | v2 `status` 已實作四態；59 項 e2e 包含「改閉包內檔案 → 失效」 |
| 2 | grader 不被受評 code 改掉 | 保住但有兩個洞（見「grader independence」） | v2 規則 4 的實作與本計畫文字有差 |
| 3 | evidence 綁精確閉包 | 保住；去掉 TTL 是對的 | 但 D9 的人工 record 只能靠簽名 + 操作者誠實，要寫成 known limit |
| 4 | 人決定 scope / invariant / known limit | 保住；`invariants` 改動觸發 approval 是今天沒有的 | v2 加了 `openQuestions` 三桶分流，本計畫應吸收 |
| 5 | 離線、repo 內、UUID | 保住 | requirement 層 UUIDv7；`legacy.zt` 索引 |

## 計畫做對的事

- **從 invariant 出發、刪除清單比新增多**：17 列刪除對應今天 117 支 `scripts/check/`、70 行 `devloop_observations.py`、559 行 `ci.yml` 與 `.tasks/admissions/` 119 個 digest，每一列都點名了檔案，不是口號。
- **evidence branch 而非 git notes** 的三個理由站得住（branch protection、O(1) 查詢、fast-forward）；v2 已照此實作。
- **「缺的才重跑」而非「diff → target 對照表」**：以 closureDigest 是否已有 record 決定重跑，不需要維護對照表，也自動涵蓋 resolver 看不到的路徑。這是對舊版（11/5 第一版）的實質修正。
- **living spec 取代 item 模式**，並且說明了退回去幾乎免費、反方向要重新歸戶。選更一般的那個是對的。
- **「第一次全跑會把 66 個 done 裡 grader 已 rot 的標成 failing」** 是這個計畫最直接的回報，也是今天 item 模式結構上做不到的體檢。
- **Phase 2 新舊並行、兩個 checker 都 required 先量觸發率**，再決定共用 lib 的例外——這是正確的順序。
- **Phase 0 量測 (2)** 正確指出 `evidence.yml` 把 gate 執行從 agent 機器搬到 CI 是成本結構的改變，而且擋的是 Phase 2 不是 Phase 5。

## 需要修改的設計點

### 1. D8：product 段的 closure digest

計畫把 closureDigest 分三段：(1) verification closure（現成）、(2) system image closure derivation id（`contracts/system-image-closure/v3`，今天靠 `--image` 手填）、(3) product 段，由 D8 決定，建議 (a) 整個 product tree 一個 digest。

問題：

- (a) 等於「改任何 product 檔 → 全部 target 的 record 失效 → 全部重跑」。今天 `just/planes-*.just` 共 85 個 recipe，44 個 `check-sel4-*plane*.py`，多數是 arm64 QEMU plane，今天 CI 完全不跑。計畫正確地把量測放在 Phase 0，但沒有給 go/no-go 的數字門檻，也沒有說量出來太慢時的 fallback 是什麼。
- (a) 與 (2) 重疊：QEMU plane 的 product 輸入就是它開機的 image，而 image 的 closure derivation 已經存在且已被 `closure_derivation_check` 保護。對 QEMU plane 來說，(2) 就是 product 段；(a) 多算的只有 host-side cargo test 類 target 的 product 輸入。

建議：

- QEMU plane target：product 段 = 它所開機 image 的 closure derivation id，由 recipe 宣告（或 checker 自報）取代 `--image` 手填。計畫已經說要把 `--image` 改成宣告，差的只是把 D8 的範圍縮到「非 image target」。
- 其餘 target（host cargo test、contracts_check 等）：採 v2 的做法——`[metadata("inputs", "<glob>")]` 宣告 + cargo recipe 自動加 crate 目錄與 workspace manifest，並用 sandbox 執行讓漏宣告的輸入直接讓 exam 失敗（`record` 把閉包檔案複製到 tmp 執行）。這就是 D8(b) 加了安全網；v2 的 e2e 已驗證「執行期組出的路徑 → exam 紅 → 宣告 inputs 後綠」。
- 保留 (a) 作為量測基線，不作為起步方案。

注意：v2 的 sandbox 在 slime_os 要處理 `deps/`（seL4 submodule）與 `scripts/build/build-sel4.py` 的輸入規模，複製整個閉包可能不可行；可退為「不複製、只用 closure 檔案清單比對實際 open() 的檔案」或接受 QEMU plane 不 sandbox（因為它的 product 段由 image derivation 保證）。這要在 Phase 1 做一個 plane 實驗確認。

### 2. grader independence 的兩個洞與一個成本

計畫文字：「PR 若修改了任何一條 acceptance 所綁 target 閉包中、在 base 上已存在的檔案，或改了任何 area 的 invariants，要人工 approval；新增的 checker 檔自由進」。

- **洞 1：recipe 本身。** 如果「閉包」只算 script token + 靜態 import，PR 可以新增 `check-foo-v2.py` 並把 recipe 改成呼叫它，舊檔沒動，不觸發。v2 明確把 recipe 定義（`just --dump` 遞迴依賴）放進 grader 集合。計畫要寫明 recipe 定義在 grader 集合內。
- **洞 2：誰的 approval。** D5(b) 「host 原生 approval rule」只擋 agent 自己加 label；若 agent 用獨立帳號且有 write 權，它可以 approve 另一個 agent 的 PR。v2 的做法是 `--approvers` ∩ `HUMAN_OWNERS`（repo 內一個檔列出人類 login），workflow 只負責從 host 取 approvers。計畫應寫成「approval 必須來自 `HUMAN_OWNERS` 內的帳號」，host 原生 rule 另外疊。
- **成本：digest 集合 ≠ grader 集合。** 計畫說 digest 要包含 `rust-toolchain.toml`、`sel4/pins.toml`（對），但若 grader 集合 = digest 集合，那 toolchain bump、`Cargo.lock` 更新、product 路徑被 checker token 掃到，全部觸發 approval。v2 的拆法：toolchain 與 product 路徑進 digest 但不進 grader 集合；`GRADER` 固定集合（`lib.py`、`record`、observation schema）永遠在兩者內。這個拆法直接決定風險段「共用 lib 幾乎每改必觸」的實際觸發率，應在 Phase 1 定下，不要等 Phase 2 量完再補。
- **v2 的 `h.contract` 例外**（契約由規則 2 保護，不再做閉包檢查）在 slime_os 對應的是 Zutai contract 的 `contracts/**` 改動。slime_os 今天沒有 `h.contract`，但 `contracts_check` 一類 target 的閉包會涵蓋大量 `contracts/`；若不拆，每個改 schema 的 PR 都觸發。建議把「改 `contracts/**/schema.zt`」獨立成一條規則（改既有 wire 格式要人 approve），與 grader 規則分開。

### 3. 刪 `depends` / `parent` 之後，backlog-first 靠什麼

今天 AGENTS.md 的規則：「`backlog` tag 的 defect 要先 resolved / deferred / blocked，才能開 milestone 工作」。計畫說依賴關係「由 target 是否存在與 `supersedes` 表達」，但這兩者都表達不了「A 要先於 B」或「這是 defect 不是 feature」。

需要補一句決策，例如：

- `failing` 是新的 backlog：main 上任何 `failing` requirement 存在時，碰到同一 area 的 PR 必須修它或把它標 `planned` 並寫 note；或
- 明說 backlog-first 規則作廢，改由 `spec status --unenforced` 的人工 review 取代。

兩者都可以，但不能沒有。D4 的 `STATUS.md` 若之後加，也只是可見性，不是機制。

### 4. CI 簽名 key 的暴露面

Phase 3 說 CI 的 ssh key 放 secret 並列在 `allowed_signers`，所有 record 都簽。風險：GitHub 上同 repo 分支的 `pull_request` workflow 看得到 secrets；有 write 權的 agent 可以在 PR 裡改 workflow 用那把 key 簽一份偽造 record 並推 evidence branch（branch protection 允許 CI token push）。

要寫進計畫：

- 簽名 key 只放在一個 GitHub Environment（GitLab 對應 protected variable），deployment branch 限 `main`，`evidence.yml` 只在 `push: main` 觸發。
- `evidence.yml` 寫入前重算 HEAD 的 closureDigest，拒收 digest 對不上的 record（v2 `publish` 已這樣做）。
- D9 的人工 `attest` record 無法防 dirty tree；寫成 known limit，狀態報表標出 `runner != ci` 的 record。
- v2 已決定「agent 用獨立帳號、不能推 evidence」；這條應抄進計畫。

### 5. D11：area 以 plane 切，還是以子系統切

計畫建議 (a) 一個 plane 一檔，約 40 檔，理由是歸戶時可以機械地用 gate target 所屬 plane 當 area。

反對理由：

- plane 是「誰檢查」，不是「系統承諾什麼」。一條 requirement 的 acceptance 常落在兩個 plane（例如 network 的 fabric 與 runtime）；以 plane 切就得挑一邊。
- `docs/architecture/` 已有 12 頁，是人維護的子系統切分，而且 AGENTS.md 已規定「boundary 行為改了要同 PR 更新 owning architecture page」。area = architecture page 讓 spec diff 與 architecture page 更新落在同一個單位，reviewer 讀一份東西。
- (b) 的「多 PR 同改一檔會衝」計畫自己已經回答了：requirement 有穩定 id、只追加不重排，衝突是可解的文字衝突。

建議：歸戶時先機械地用 plane 當暫時 area（取其可自動化），Phase 3 人核時合併成 architecture page 粒度。或至少把 D11 列為「Phase 3 人核時可重切」，而不是 Phase 1 就鎖死。

### 6. 歸戶的機械性沒有計畫寫的那麼高

我數了 104 個 dev-spec/v1 body 的 predicate：`passed` 851、`any` 166、`all` 165、`count` 160、`equals` 22。計畫的 helper 只有 `count / atLeast / passed / all`，少 `any` 與 `equals`。要嘛 helper 補上，要嘛 `spec migrate` 要人工改寫這 188 條。另外 body 裡有 `problem`、`scope`、`nonGoals` 欄位，計畫的 `Area` 沒有對應位置；`nonGoals` 是人擁有的 scope 決定（HUMAN.md），丟掉它等於丟掉 invariant 4 的一部分記錄。建議 `Area` 加 `nonGoals`，或歸戶時把它寫成一條 `h.decision`。

### 7. Phase 1 的離場條件是一座拋棄式的橋

「`just spec_gen` 把 area 檔算成每條 requirement 的 `.zti`，歸戶自舊 item 的那條通過現有 devloop validate」——這要求新格式能 render 成 dev-spec/v1，而 dev-spec/v1 在 Phase 3 刪除。這座橋只用一次。它確實能證明等價性，但 Phase 2 的離場條件（真實功能走完、無 bot commit）已經是更強的測試。建議把 Phase 1 離場條件改為「`spec check` 通過 + `requirementsDigest` 對同一輸入穩定 + 一個 plane 的 closureDigest 算得出來」，省下這座橋。若要保留等價性驗證，只做 digest 比對，不做整份 `.zti`。

### 8. 工時

4.5 週偏樂觀，主要在 Phase 3：

- 104 個 item 歸戶 + 63 個 prose item 逐一判斷 + 人核 area 屬歸與 `planned` 標記，一個 PR。v2 greenfield 寫 CLI 花的量級與計畫一致（`scripts/spec/` 1.3k 行），但 v2 沒有遷移。
- 43 份 docs 提到 devloop / myque，`docs_check` 會揪出連結，但改寫是人要讀的。
- Phase 2 並行期間，每個碰 `scripts/` 的 PR 都改 `identity.code`，舊 loop 的 active item 不斷 re-record（計畫 D3 已指出）。這是 Phase 2 的 1.5 週裡隱形的拖累。

建議 Phase 3 估 1 週，總計 5.5 週；或把 63 個 prose item 的逐一判斷移出 critical path（全部只留索引，之後要做再手寫）。

## 與 v2 實作的差異對照

v2 是空 repo 上的模型驗證，跳過 Phase 0–3；下面是 v2 做了而本計畫沒有、且應該吸收的：

| v2 有、計畫沒有 | 為什麼該吸收 |
| --- | --- |
| digest 集合 / grader 集合分開 | 直接決定 approval 觸發率，見第 2 點 |
| `[metadata("inputs")]` + sandbox 執行 | D8 的安全網，見第 1 點 |
| recipe 定義進 grader 集合 | 封「新增檔 + 改 recipe」的洞 |
| `HUMAN_OWNERS` 檔 + `--approvers` 從 host 傳入 | host-neutral 且封 agent 互相 approve |
| `publish` 拒收 digest 對不上 HEAD 的 record | 防 dirty tree record |
| agent 獨立帳號、不可推 evidence | 防偽造機器 record |
| `openQuestions` 三桶分流（可量測 / 可逆預設 / 人擁有） | 不讓 reviewer 讀到「你選」噪音；byHuman / byAgentDefault 比例要是真數字 |
| `scripts/spec/` 行數上限（1500）由 `check` 強制 | 防新 CLI 長回今天的 devloop |
| `status` 印 product vs process commit 比例 | 計畫的量化目標（repo/tasks → 0）要有人印出來 |

計畫有、v2 刻意沒做、要決定的：

| 計畫有、v2 沒有 | v2 的理由 | 建議 |
| --- | --- | --- |
| `mandatory` 欄位 / `advisory` 狀態 | 沒用途前不加 | 跟 v2：不加。要「軟」acceptance 就用 `planned` |
| `h.all {…}` 遞迴 predicate | 一個 list 就是全部成立 | 跟 v2；但 slime_os 需要 `any`（166 處），得加 |
| `render`、`spec_gen --check` 產 `.zti` | 沒人消費 `.zti` | 跟 v2：`check --digest` 印 digest 即可 |
| Zutai pin 放 `sel4/pins.toml` | `sel4/pin.toml` 在每個 digest 裡，Zutai bump 會 stale 全部 | 計畫自己也說 Zutai 不進 digest；放 `sel4/pins.toml` 就要把它從 digest 的 toolchain 段裡拆出 Zutai 那一列，不如分檔 |

## 文件層次的不一致與數字

| 位置 | 問題 | 修正 |
| --- | --- | --- |
| p.3 流程圖 | 標題與第三 lane 仍寫「git notes / `refs/notes/evidence`」 | 改成 evidence branch |
| p.1 invariant 2 | 「non-done spec 所綁閉包」，但內文已改為「所有 requirement、沒有 done」 | 表格同步 |
| p.1 / p.2 | `codePaths` 寫 311，實際 `.devloop/policy.json` 有 312 | 小 |
| Phase 0 | `.devloop/inputs` 「約 50 份」，實際 75 | 量測 (2) 規模 +50% |
| 風險段 | 「五十幾個 `check-sel4-*-plane.py`」，實際 44；plane recipe 85 | 小 |
| 目標 | repo/tasks 126/200，我算 123/200 | 可接受 |
| D 編號 | 無 D10 | 補或重編 |
| helpers | 缺 `any`、`equals` | 見第 6 點 |
| `Area` | 無 `nonGoals` / `problem` 對應 | 見第 6 點 |

## 建議的開工順序（修正後）

1. **先回答 D8（改採 v2 方案）、D5（加 `HUMAN_OWNERS`）、D11（暫以 plane、Phase 3 可重切）**，其餘 D 照計畫建議。
2. **Phase 1 加兩件事**：定下 digest / grader 兩個集合；在一個 arm64 QEMU plane 上試 closureDigest 三段都算得出（含 image derivation id 由 recipe 宣告），並記錄一次 wall time。
3. 其餘 phase 照計畫，Phase 3 估 1 週。

## 沒有驗證的事

- 沒有跑 v2 的 `just spec-e2e`，59 項 e2e 的結果以 v2 文件為準。
- 沒有量任何 wall time；Phase 0 的量測沒有替代品。
- 沒有檢查 vsdtdd reader 的現況。
