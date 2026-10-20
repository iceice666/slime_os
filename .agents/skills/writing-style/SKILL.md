---
name: writing-style
description: Rules for any prose an agent writes in Slime OS — PR bodies first, then conversation replies, then `docs/`, then work-item text. Combines ISO 24495-1 (reader-first structure), ASD-STE100 (countable sentence and word rules, with `docs/glossary.md` as the controlled vocabulary), a bounded form of ELI5 (analogies only as entry points), and Mayer's multimedia principles (when a table or diagram helps and when it adds load). Use before writing or reviewing a PR description, a reply to a human, a page under `docs/`, a work-item body, or a code comment longer than one line. Bilingual: every rule is stated in English and in Traditional Chinese.
---

# Writing style / 寫作規範

Most text in this repository is written by agents and read by a human who is
reading many such texts a day. The goal is to lower the cost of
understanding, not to sound simple. Precision wins over simplicity when they
conflict; the glossary term wins over the plain word.

本 repo 大多數文字由 agent 寫、由一天要讀很多篇的人類讀。目標是降低理解成本，
不是聽起來簡單。精確與簡單衝突時，精確優先；術語表的詞優先於白話詞。

Scope and priority / 適用範圍與優先順序:

1. PR body (`.github/PULL_REQUEST_TEMPLATE/`)
2. Conversation replies to a human
3. `docs/`
4. Work-item bodies under `.tasks/`

`change-records` says *where* a fact is recorded. `pr-review` says what a
review may say. This skill says *how* the sentences are written.

## Language / 語言

- PR bodies, `docs/`, work items, commits, and code comments are English.
  / PR body、`docs/`、work item、commit、程式註解一律英文。
- A conversation reply uses the language the human wrote in. Keep identifiers,
  paths, commands, and glossary terms in their original form; do not translate
  `capability`, `generation`, `closure`, or any `docs/glossary.md` entry.
  / 對話回覆用人類使用的語言。識別字、路徑、指令、術語表的詞保留原文，不翻譯。
- Do not mix languages inside one sentence except for those untranslated
  tokens. / 同一句話不混用兩種語言，上述保留原文的詞除外。

## 1. Reader first (ISO 24495-1, principle 1) / 先想讀者

Before writing, answer three questions; the answers decide the content.
下筆前先回答三個問題，答案決定內容。

| Text | Reader | Reader's question |
|---|---|---|
| PR body | reviewer, then the person merging | What becomes true, what could break, what was actually run? |
| Reply | the human who asked | What is the answer, what do I decide, what do I run? |
| `docs/` page | a contributor who has not read the code | What is this, where is it owned, what must stay true? |
| Work item | the next agent to pick it up | What is in scope, what proves it done? |

- Include only what that reader needs to act. Investigation history, dead
  ends, and reasons the author was right are not content; `change-records`
  says where they go if anywhere.
  / 只放讀者行動所需的內容。調查過程、死路、作者為什麼對，都不是內容。
- A declared limit is content. Say what was not done or not verified.
  / 已知限制是內容。明說什麼沒做、什麼沒驗證。

## 2. Findable structure (ISO 24495-1, principle 2) / 可找到的結構

- Conclusion first. The first sentence of a PR body, a reply, or a section
  states the result; the reasoning follows.
  / 結論先行。PR body、回覆、章節的第一句就是結果，理由在後面。
- A heading predicts its section. A reader who reads only headings must get
  the outline. / 標題要能預測內容。只看標題也要能得到大綱。
- One idea per paragraph, at most six sentences. / 一段一個想法，最多六句。
- Keep supplementary material out of the main path: a `<details>` block, a
  footnote-style "Known limits" section, or a link.
  / 補充資料不放主線：用 `<details>`、獨立的 "Known limits" 段，或連結。
- When the human must decide something, put the decision in its own list at
  the end, one item per decision, each with the options.
  / 需要人類決定的事，放在結尾的獨立清單，一項一個決定，附上選項。
- Follow the PR template's section order; do not add sections before `Why`
  or `Claim`. / 依 PR 模板的段落順序；不要在 `Why` / `Claim` 前面加段落。

## 3. Sentences (ASD-STE100 Part 1, applied) / 句子規則

These are countable. A reviewer may quote the rule number.
這些規則可計數。reviewer 可直接引用規則編號。

| # | Rule | 規則 |
|---|---|---|
| S1 | At most 25 words per sentence; at most 20 in a step of a procedure. A code span, path, or number with unit counts as one word. | 一句最多 25 個英文字；程序步驟最多 20 字。code span、路徑、帶單位的數字算一個字。中文一句最多 40 字。 |
| S2 | One statement per sentence. No semicolons; no dash joining two statements. | 一句一個陳述。不用分號，不用破折號連接兩個陳述。 |
| S3 | Active voice. Name the actor: `slime-root refuses the manifest`, not `the manifest is refused`. Passive only when the actor is unknown or irrelevant. | 主動語態，寫出動作者。只有動作者未知或無關時才用被動。 |
| S4 | Verbs for actions, not nouns: `the loader fails`, not `a failure of the loader occurs`. | 動作用動詞，不用名詞化。 |
| S5 | Imperative for instructions: `Run just test`, not `You can now run just test`. | 指令用祈使句。 |
| S6 | Condition before action: `If the digest differs, the check refuses the tree.` | 條件在前，動作在後。 |
| S7 | `must` for a requirement, `does` for a fact, `may` for permission. Do not use `should`. | 要求用 must、事實用 does、允許用 may。不用 should。 |
| S8 | No contractions, no filler openers (`Note that`, `It is worth mentioning`, `Basically`). | 不用縮寫，不用填充開頭（「值得一提」「基本上」）。 |
| S9 | Present tense for what the code does now; past tense only for what was observed (`the run printed`). | 現在式描述程式現況；過去式只用於觀察到的事實。 |
| S10 | Numbers, names, and commands are exact. `three checkers`, not `several`; `just docs_check`, not `the docs check`. | 數字、名稱、指令要精確。不用「幾個」「相關檢查」。 |

## 4. Words (ASD-STE100 Part 2, with the glossary as dictionary) / 詞彙規則

STE's own dictionary is for aerospace maintenance. Here the controlled
vocabulary is `docs/glossary.md`; the rules below are STE's word rules
applied to it.

STE 自己的字典是為航太維修寫的。這裡的受控詞彙是 `docs/glossary.md`；
以下是把 STE 的用字規則套用在它上面。

| # | Rule | 規則 |
|---|---|---|
| W1 | One concept, one word. Use the glossary term and only that term. Do not rotate synonyms for variety (`component` / `service` / `process` are three different things, not one). | 一個概念一個詞。只用術語表的詞，不為了變化而換同義詞。 |
| W2 | A term outside the glossary gets a definition or a link at its first use in the text. | 術語表以外的詞，第一次出現要定義或給連結。 |
| W3 | A word has one meaning in one text. If `check` is a script, do not also use it as a verb in the same section. | 同一篇文字裡一個詞只有一個意思。 |
| W4 | Prefer the shorter common word when the glossary does not own the concept: `use`, not `utilize`; `start`, not `initiate`; `because`, not `due to the fact that`. | 術語表沒有的概念用常見短字。 |
| W5 | No phrasal verbs where a single verb exists: `install`, not `set up`; `remove`, not `take out`. | 有單一動詞就不用片語動詞。 |
| W6 | Noun chains of at most three words. `boot layout fixture` is the limit; split anything longer. | 名詞串最多三個字。 |
| W7 | No hedging adverbs that carry no information: `basically`, `simply`, `just` (the adverb), `essentially`, `very`. | 不用無資訊量的修飾詞。 |
| W8 | No evaluative adjectives about the work: `robust`, `clean`, `elegant`, `comprehensive`. State the property instead. | 不用評價工作的形容詞，改寫成具體性質。 |

## 5. Explaining (ELI5, bounded) / 解釋規則（受限的 ELI5）

An explanation fails when it starts in the middle or defines jargon with
jargon, not when the words are long. Analogies are entry points, never
definitions.

解釋失敗多半是從中間講起、用術語解釋術語，而不是字太難。
類比是入口，不是定義。

- Foundation first: state what the thing is, then the problem, then the
  mechanism. / 先說這是什麼，再說問題，再說機制。
- An analogy is allowed only in `docs/concepts/`, `docs/getting-started/`, and
  conversation replies. It is not allowed in a PR body, an architecture page,
  a decision record, or a work item.
  / 類比只能出現在 `docs/concepts/`、`docs/getting-started/` 和對話回覆。
- Every analogy sits next to the precise statement or a link to it, and names
  where the analogy stops holding.
  / 每個類比旁邊必須有精確陳述或連結，並說明類比在哪裡失效。
- Never trade a capability, authority, determinism, or verification boundary
  for a simpler sentence. If the simple version is wrong, write the precise
  one. / 為了句子簡單而犧牲 capability、authority、確定性、驗證邊界的精確度，
  是不允許的。

## 6. Tables and diagrams (Mayer) / 表格與圖

Words plus a well-placed picture lower load; a decorative picture raises it.
Pick the form by the shape of the information.

文字加上放對位置的圖會降低負擔；裝飾性的圖會增加負擔。依資訊的形狀選形式。

| Information shape | Use | 用 |
|---|---|---|
| N items × M attributes | Markdown table | 表格 |
| Ordered steps | Numbered list | 編號清單 |
| Who calls whom, what flows where | Mermaid fenced block (`flowchart` or `sequenceDiagram`) | Mermaid |
| One fact | A sentence | 一句話 |

- A diagram sits directly under the paragraph it illustrates, never in an
  appendix. / 圖緊跟在它說明的段落下面，不放附錄。
- A diagram carries at most 12 nodes. Split anything larger.
  / 一張圖最多 12 個節點。
- No raw HTML for diagrams in `docs/` or PR bodies: GitHub drops `<style>`,
  `<script>`, and inline `<svg>`. Allowed HTML is `<details>`, `<table>`,
  `<kbd>`, `<sup>`, and `<img>` to a checked-in `.svg`.
  / `docs/` 和 PR body 不用原生 HTML 畫圖，GitHub 會剝掉。允許的 HTML 只有
  `<details>`、`<table>`、`<kbd>`、`<sup>` 和指向 `.svg` 檔的 `<img>`。
- Custom HTML charts are allowed only in a conversation reply rendered by a
  client that supports them; otherwise fall back to a table.
  / 自定義 HTML 圖表只能用在支援它的對話介面，否則退回表格。
- A table has at most 7 rows before a sort order or a split is required.
  / 表格超過 7 列就要排序或拆分。

## 7. Per-text checklists / 各類文字的檢查表

### PR body

1. `Why` or `Claim` is at most three sentences and states what becomes true.
   / `Why` 或 `Claim` 最多三句，說明什麼變成真。
2. `What changed` lists files or areas, not a narrative.
   / `What changed` 列檔案或區域，不是敘事。
3. `Verification` names the exact command, the observed result, and the
   evidence class. Nothing that was not run appears as passed.
   / `Verification` 寫確切指令、觀察結果、證據類別。沒跑的不寫成通過。
4. Known limits are stated in one list, not scattered.
   / 已知限制集中在一個清單。
5. No sentence over 25 words (S1); no evaluative adjective (W8).

### Conversation reply / 對話回覆

1. First sentence is the answer or the result. / 第一句是答案或結果。
2. Decisions for the human are a separate numbered list at the end.
   / 需要人類決定的事放結尾的獨立編號清單。
3. Commands the human must run are in a fenced block, one per line.
   / 要人類跑的指令放 code block，一行一條。
4. Length matches the question: a yes/no question gets one paragraph.
   / 長度對應問題：是非題只回一段。

### `docs/` page

1. The first paragraph says what the page owns and links the owning code or
   contract. / 第一段說這頁擁有什麼，並連結擁有它的程式或契約。
2. Every glossary term is used as the glossary defines it (W1, W3).
3. Analogies only where section 5 allows.
4. A diagram, if any, follows section 6.

### Work item / 工作項

1. Scope is a list of what is in and what is out.
   / 範圍是「包含」與「不包含」的清單。
2. Each acceptance is one observable sentence (S2, S10).
   / 每個驗收條件是一句可觀察的陳述。

## Examples / 範例

| Before | After |
|---|---|
| It is worth noting that the manifest validation logic has been significantly refactored in order to provide more robust handling of edge cases. | `validate_manifest` now refuses a manifest whose slot count exceeds the declared ceiling. |
| The tests should pass after the change is applied. | `just test` passed on `a1b2c3d` (direct). |
| Basically the root task is like a kernel but not really. | `slime-root` is the first user task. It owns every capability the kernel hands out at boot and grants subsets to components. It is not the kernel: the kernel is seL4. |
| 這個改動讓整體流程變得比較乾淨而且更有彈性。 | `generate-system-image-closures.py` 現在拒絕含有 gitignored 檔案的工作樹。 |
