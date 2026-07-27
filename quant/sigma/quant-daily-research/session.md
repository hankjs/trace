# Session: A Stock Daily Research System

## Learner Profile

- Level: Beginner, with basic stock trading and candlestick experience
- Language: Chinese
- Started: 2026-07-27
- Goal: Learn the system's main scenarios through user stories and form a repeatable daily research workflow

## Concept Map

| # | Concept | Prerequisites | Status | Score |
|---|---------|---------------|--------|-------|
| 1 | Data clock: research date vs intraday snapshot | - | mastered | 88% |
| 2 | System candidates: ranking, score, and factor details | 1 | mastered | 86% |
| 3 | Single-stock research: candlesticks and strategy signals | 1, 2 | mastered | 84% |
| 4 | Combination screening: pool, conditions, and coverage | 1, 2 | mastered | 100% |
| 5 | Strategy validation: backtests, drawdown, and bias | 1, 3 | mastered | 93% |
| 6 | Manual decisions: watchlist, ledger, and review | 1, 3, 5 | mastered | 80% |

## Session Metrics

- Diagnostic questions: 2
- Teaching questions: 68
- Concepts mastered: 6 / 6

## Session Log

- [2026-07-27] Diagnosed as a beginner: understands basic candlesticks and real-world buying/selling, but is unfamiliar with indicators beyond candlesticks.
- [2026-07-27] Demonstrated the correct initial research instinct: inspect why a stock was selected and which date the data belongs to, then consult historical results.
- [2026-07-27] Started Concept 1: distinguish the research data date from the intraday display timestamp.
- [2026-07-27] Concept 1 round 1: correctly selected the 2026-07-24 research date and rejected using the intraday rise as direct evidence; partial gap remains around causality and the fact that strategy signals are not entry instructions (75%).
- [2026-07-27] Concept 1 round 2: correctly explained that the 2026-07-24 candidate generation did not use 2026-07-27 data, and identified a strategy signal as a research state change rather than an instruction. Concept mastered (88%).
- [2026-07-27] Started Concept 2: interpret system candidate ranking, composite score, and factor details.
- [2026-07-27] Concept 2 round 1: correctly interpreted rank as relative performance under the day's scoring rules and "new" as newly entering the candidate list (100% for this round).
- [2026-07-27] Concept 2 round 2: indicator-to-question mapping is not yet clear; cumulative score adjusted to 50%. Narrowed the next round to price movement intuition for 20-day momentum.
- [2026-07-27] Concept 2 round 3: correctly inferred a 12% price rise from 10.0 to 11.2 and mapped 20-day momentum to price change. Cumulative score: 67%.
- [2026-07-27] Concept 2 round 4: correctly interpreted RSI 78 as strong recent upward pressure with possible overheating; the comparison question was unanswered. Cumulative score: 63%.
- [2026-07-27] Concept 2 round 5: correctly identified the persistently rising stock as having the higher RSI, but described RSI as deviation from an expected trend. Partial credit; clarified that RSI has no forecast baseline. Cumulative score: 65%.
- [2026-07-27] Concept 2 round 6: correctly identified RSI as a comparison of recent upward and downward price strength, then asked how that strength is derived. Cumulative score: 68%.
- [2026-07-27] Concept 2 round 7: correctly summed positive close-to-close changes as 1.2 and absolute negative changes as 0.2 in a simplified RSI window. Cumulative score: 72%.
- [2026-07-27] Concept 2 round 8: correctly calculated relative strength RS = 1.2 / 0.2 = 6 and interpreted it as upward strength being six times downward strength. Cumulative score: 75%.
- [2026-07-27] Concept 2 round 9: correctly calculated the simplified RSI as 85.7 and interpreted it as strong upward pressure. Cumulative score: 78%.
- [2026-07-27] Concept 2 round 10: correctly interpreted volume ratio 1.8 as 1.8 times recent average volume and explained that trading activity alone does not establish a buy case. Cumulative score: 82%; mastery synthesis started.
- [2026-07-27] Concept 2 mastery check: correctly described Stock A as strong but potentially overheated, and recognized that Stock B's lower rank does not make it categorically worse. The remaining need for candlestick location, price-volume context, volatility, and signal reasons belongs to single-stock research. Concept mastered (86%).
- [2026-07-27] Started Concept 3: continue from the candidate table into the single-stock page and separate observations from strategy state changes.
- [2026-07-27] Concept 3 round 1: correctly read a high-volume long-upper-shadow candle near recent highs as increased disagreement and possible selling pressure. The explanation focused on open-close distance; the next step is to compare the close with the session high and ask who controlled the close. Score: 75%.
- [2026-07-27] Concept 3 round 2: correctly distinguished buyer-controlled and seller-pressured closes. Correctly associated volume ratio with trading scale, but repeated market activity as what volume cannot explain; the missing distinction is price direction/outcome. Score remains 75%.
- [2026-07-27] Concept 3 round 3: selected option A but then correctly stated that volume ratio cannot determine price direction or which side prevailed. Treated as a selection slip with partial credit. Cumulative score: 72%.
- [2026-07-27] Concept 3 round 4: self-corrected to option B, completing the distinction between trading activity and price outcome. Cumulative score: 76%.
- [2026-07-27] Concept 3 round 5: correctly kept the 2026-07-24 signal valid for that day's data while recognizing that 2026-07-27 conditions must be re-evaluated, especially after visible selling pressure. Asked whether daily strategies are probabilistic and whether the system provides entry ranges. Cumulative score: 80%; timing boundary check started.
- [2026-07-27] Concept 3 round 6: correctly characterized the strategy as a fixed-rule probabilistic process rather than certainty. Proposed a directly actionable entry quote instead of identifying the current research-threshold capability; this conflicts with the product's research/manual-trading boundary and requires reframing. Cumulative score: 79%.
- [2026-07-27] Concept 3 mastery check: selected a transparent research range over an actionable quote, required calculation basis, data date, invalidation conditions, and backtest context, and independently distinguished profit-taking from other exit reasons. Concept mastered (84%).
- [2026-07-27] Started Concept 4: build a structured screen from a research hypothesis and inspect data coverage.
- [2026-07-27] Prerequisite recall before Concept 4: correctly classified the example as a strategy-rule exit rather than profit-taking or stop-loss, and explained that the future trigger time and exit price cannot be known at entry.
- [2026-07-27] Learning paused at the start of Concept 4 to clarify a future requirement for strategy price references and exit guidance. No development authorized.
- [2026-07-27] Confirmed full strategy coverage, optional price ranges only when objectively calculable, and optional take-profit parameters that must participate in backtesting. Requirements documented at `docs/strategy-research-plan.md`; learning resumed at Concept 4.
- [2026-07-27] Concept 4 round 1: correctly recognized that an AND screen with only 65 jointly evaluable stocks produces 5 matches from those 65, while the remaining pool includes many unknowns rather than automatic failures. Score: 100%.
- [2026-07-27] Concept 4 round 2: correctly applied OR between condition groups and AND within each group; recognized that independent hits from incomplete groups cannot be combined into a valid match. Score remains 100%.
- [2026-07-27] Concept 4 round 3: correctly chose historical index membership for a 2022 study and explained that using today's surviving members would make historical performance look artificially better. Score remains 100%; mastery synthesis started.
- [2026-07-27] Concept 4 mastery check: accurately described 8 matches as coming from 90 fully evaluable stocks rather than treating all other pool members as failures, and predicted that changing outer AND to OR can only expand or preserve the result set. Concept mastered (100%).
- [2026-07-27] Started Concept 5: evaluate strategy return together with drawdown, execution assumptions, and bias.
- [2026-07-27] Concept 5 round 1: preferred the lower-return strategy because its maximum drawdown better matched the learner's risk tolerance, and rejected judging a strategy by return alone. Score: 100%.
- [2026-07-27] Concept 5 round 2: correctly calculated a decline from 160,000 to 72,000 as 88,000 or 55% of the peak, and distinguished maximum drawdown from loss probability. Score remains 100%.
- [2026-07-27] Concept 5 round 3: correctly required T+1 open execution for a close-formed signal and recognized that same-day open execution would make the backtest look artificially better by capturing an unavailable move. Score remains 100%.
- [2026-07-27] Concept 5 round 4: correctly identified high-turnover strategies as more vulnerable to ignored costs, but read the remaining 0.15 percentage points as 15% rather than 0.15%. Cumulative score: 88%.
- [2026-07-27] Concept 5 round 5: correctly converted 0.15% of 10,000 to 15 and 15% to 1,500, resolving the percentage-scale error. Cumulative score: 90%; mastery synthesis started.
- [2026-07-27] Concept 5 mastery check: identified parameter overfitting and proposed validating selected parameters on later unseen data; chose a separated training and test process. Concept mastered (93%).
- [2026-07-27] Started Concept 6: keep research signals, external manual trades, and the internal position ledger distinct but traceable.
- [2026-07-27] Concept 6 round 1: correctly rejected automatic position changes from strategy prompts and identified external trade facts such as price, quantity, fees, and total cost as the source of the manual ledger. Score: 100%.
- [2026-07-27] Concept 6 round 2: correctly calculated total buy cost as 12,305 and average cost as 12.305 per share; the partial-sale question was unanswered. Cumulative score: 75%.
- [2026-07-27] Concept 6 round 3: correctly calculated that 600 shares remain after selling 400, but treated sale proceeds as a reduction of the remaining position's historical cost. The next round separates cost assigned to sold shares from sale proceeds and realized profit or loss. Cumulative score: 67%.
- [2026-07-27] Concept 6 round 4: correctly assigned 4,922 of historical cost to the 400 sold shares, retained 7,383 as the cost of the remaining 600 shares, and showed that their average cost remains 12.305. Cumulative score: 75%; synthesis check started.
- [2026-07-27] Concept 6 mastery check: correctly calculated net sale proceeds of 5,194 and realized profit of 272, then explained that a strategy exit prompt must not change the ledger before an actual external trade is manually recorded. Concept mastered (80%).
- [2026-07-27] Completed all six user-story scenes. The learner can now follow the full daily workflow from checking the data clock through research, validation, manual action, ledger accounting, and review.
