# devloop 重設計 — 決策紀錄

2026-11-07。人說「要決定的都先你選」。依 v2 `devloop.md` 的規則：這些是 **byAgentDefault，delegated by human 2026-11-07**；不是 byHuman。每條附 reopen 訊號。人隨時可以翻。

格式：`Decision: <option>. Because <one sentence>. Reopen if <signal>.`

## 計畫內的 D1–D11

| # | 決策 | 理由 | Reopen if |
| --- | --- | --- | --- |
| D1 | (a) submodule 是 Zutai 唯一真相，`nix/zutai.nix { src = ./deps/zutai }`，eval 帶 `?submodules=1` | v2 已這樣跑，離線可建，GitLab 不用 token | CI 的 recursive checkout 讓 pipeline 多 >2 分鐘 |
| D2 | (a) `ssh-keygen -Y sign`，`specs/allowed_signers` 進 repo，namespace 固定 `slime-human` | 不依賴 host，v2 已驗簽名、竄改、未知簽名者三個負面案例 | 有第二個人要簽而 key 管理成為阻力 |
| D3 | (b) 5 個 active item 立即歸戶為新格式 | Phase 1–2 每個碰 `scripts/` 的 PR 都改 `identity.code`，留在舊 loop 會一直 re-record | 任一 active item 兩天內就能走舊流程收尾 |
| D4 | (a) 純計算，不回寫 `.zt`；`spec status` 另印 product vs process commit 比例 | 回寫就是 bot commit，正是要刪的東西 | 一個月後有人要在 `git log` 看進度 |
| D5 | (b) host 原生 approval rule **疊** `HUMAN_OWNERS`：`check --approvers` 取 host approvers，必須 ∩ `HUMAN_OWNERS` 非空 | 單靠 host rule 擋不住 agent 帳號互相 approve；v2 已實作 | `HUMAN_OWNERS` 超過 3 人且與 host CODEOWNERS 重複維護 |
| D6 | (a) SDK release repo 留 GitHub，URL 集中成一個常數 | 消費者位置未知，先不動 | Phase 5 時確認消費者全在 GitLab |
| D7 | (b) 無 bot，人審；AGENTS.md 的「Codex P2」改泛稱 | 未定就先選最便宜的；Phase 5 才碰 | 人審每週 >5 個 PR 吃不下 |
| D8 | **(b) 的 v2 版**：QEMU plane 的 product 段 = 所開機 image 的 closure derivation id（recipe 宣告，取代 `--image`）；非 image target 用 `[metadata("inputs", glob)]` + cargo crate 規則；(a) 只當 Phase 0 量測基線 | (a) 讓每次 main push 重跑 85 個 plane；image derivation 已存在且受 `closure_derivation_check` 保護 | Phase 1 試點 plane 的 derivation id 無法由 recipe 宣告取得 |
| D9 | (a) `just attest <target>` 寫 ssh 簽名 record 推 evidence branch；report 標 `runner != ci`；tree 內 `evidence/` 轉 history archive | evidence 只有一個家；dirty tree 防不了，寫成 known limit | 一份 attest record 被發現對不上宣稱的 digest |
| D11 | 歸戶時機械地以 plane 當 area（`just/planes-*.just` 分組）；Phase 3 人核時合併到 `docs/architecture/` 12 頁的粒度 | plane 可自動化，architecture page 才是「承諾什麼」且 AGENTS.md 已綁 boundary 改動要更新它 | 合併後單一 area 檔超過 ~60 條 requirement |

## 評估中新增的四點

| # | 決策 | 理由 | Reopen if |
| --- | --- | --- | --- |
| E1 | digest 集合 ≠ grader 集合，Phase 1 定死：**digest** = recipe 定義 + 閉包檔 + `inputs` + toolchain 檔（`rust-toolchain.toml`、`sel4/pins.toml`、`flake.lock`、`Cargo.lock`）+ `GRADER` 固定集合；**grader** = recipe 定義 + 閉包內 `scripts/` 檔 + `GRADER`，排除 product 路徑與 toolchain 檔 | toolchain bump 與 product 改動不該要 approval；recipe 必須在 grader 內才封「新增檔 + 改 recipe」 | Phase 2 並行量到 approval 觸發率 >30% 的 PR |
| E2 | `contracts/**/schema.zt` 既有內容改動獨立成規則 2'，需 `HUMAN_OWNERS` approve；與 grader 規則分開 | wire 格式是不可逆、人擁有的；不拆則每個 schema PR 走 grader 出口，訊號混掉 | 只改註解的 schema PR approve 多到變噪音 |
| E3 | backlog-first 改為：main 上 `failing` 的 requirement 由同 area 的下一個 PR 修掉或標 `planned` + note；`spec check --base` 擋「碰到該 area 但兩者都沒做」的 PR | 唯一能機械檢查的替代；`depends`/`parent` 不搬 | 一個 area 累積 >5 條 failing 而無人碰 |
| E4 | CI 簽名 key 放 GitHub Environment `evidence`（deployment branch 限 `main`）；`evidence.yml` 只在 `push: main` 觸發；寫入前重算 HEAD closureDigest 拒收不符 record；agent 帳號不可推 `evidence` | 封 PR workflow 偽造 record | 有 fork PR 需要寫 evidence |

## vsdtdd 對照新增（詳見 `vsdtdd-mapping.md`）

| # | 決策 |
| --- | --- |
| V1 | requirement 選填 `depends`，只給 viewer 與排序，不進 digest、不影響四態 |
| V2 | area 選填 `constrainedBy: [docs/decisions/<file>]`；`docs/decisions/` 檔名不改 |
| V3 | vsdtdd reader 只讀 `specs/*.zt` 與 `spec status --json`，不碰 evidence branch；status 輸出用 vsdtdd 的四個狀態名，`planned` 報成 `advisory` + `reason` |

## 評估中其他建議的取捨

| 項目 | 決策 |
| --- | --- |
| `mandatory` / `advisory` | 不加；跟 v2。「軟」acceptance 用 `planned` |
| helper 集合 | `count`、`atLeast`、`atMost`、`flag`；`passed` 隱含；list 即 all。Phase 1 實測舊 body 沒有 `any`，不加（見 `phase1-log.md`） |
| `Area` 欄位 | 加 `nonGoals`（list of text）；歸戶時 `problem` 併入 `rationale`，`scope` 丟掉（已被 requirements 表達） |
| `render`、`spec_gen` 產 `.zti` | 不做；`check --digest` 印 requirementsDigest |
| Zutai pin 位置 | `specs/zutai.toml`，不進 digest；不併入 `sel4/pins.toml` |
| Phase 1 離場條件 | 改為：`spec check` 綠、同輸入 digest 穩定、一個 arm64 QEMU plane 的三段 closureDigest 算得出並記一次 wall time；不做 `.zti` 等價橋 |
| 63 個 prose item | 全部只留索引，移出 critical path；要做再手寫 |
| Phase 3 工時 | 1 週；總計 5.5 週 |
| sandbox | QEMU plane 不 sandbox（product 段由 image derivation 保證）；host target sandbox 在 Phase 1 用一個 cargo test target 試，不可行則退為「比對 closure 清單與實際 open() 檔案」 |
| `scripts/spec/` 行數上限 | 1500，`check` 強制 |

## 文件修正（直接改 `plan.md` 時套用）

- p.3 圖與 invariant 2 表格改為 evidence branch / 全部 requirement。
- `.devloop/inputs` 75、plane checker 44、plane recipe 85、codePaths 312、repo/tasks 123/200。
- 補 D10 編號或重編。

## 下一步（尚未執行）

第一步照計畫：D3 + D11 一起做——挑一個 active item，歸戶成 `specs/<plane>.zt`，把同 plane 的 done item 歸進同一檔，看一個完整 area 檔長什麼樣子；同時在該 plane 上驗 D8 / E1 的 digest 算得出。
