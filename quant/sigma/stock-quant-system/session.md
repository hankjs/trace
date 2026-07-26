# Session: 用量化系统学习股票投资

## Learner Profile

- Diagnosed level: beginner with basic market vocabulary
- Language: Chinese (zh-CN)
- Started: 2026-07-26
- Learning goal: use this system for stock selection, backtesting, and portfolio simulation; iterate the system when research needs expose gaps
- Product boundary: daily-frequency research and decision support only; all real trades remain manual and external
- Learning preference observed: concrete examples first, with short explanations and system practice
- Risk tolerance: not yet calibrated; learner currently expresses strong concern about principal loss
- Time horizon and weekly study time: not yet collected

## Diagnostic Evidence

- Stocks: recognizes shares issued by listed companies and secondary-market trading
- Funds: recognizes pooled money managed by a professional institution, but fund structure and risk ownership are not yet tested
- Indices: recognizes an index as a market indicator, but currently treats CSI 300 as the total value of the top 300 Shanghai/Shenzhen stocks
- Daily bars: can identify open, close, high, low, and price direction; volume and adjusted prices are not yet tested
- Financial statements: can name the balance sheet and cash-flow statement; income statement and statement relationships are not yet tested
- Valuation and quality: PE, PB, and ROE are unfamiliar
- Performance: reads annualized return literally as a one-year return; compounding and comparability are not yet established
- Risk: interprets a 30% maximum drawdown as a 30% probability of loss; drawdown path and loss probability are not yet distinguished
- System practice: intends to use selection, backtesting, and portfolio simulation, then improve the software as research needs become clearer

## Current Concept

- Concept: Session complete
- Status: mastered
- Score: 100% roadmap completion
- Questions: 74 learning responses across 11 concepts
- Evidence: demonstrated a complete data-check, signal-review, backtest-review, portfolio-risk, manual-decision, and record-keeping workflow while preserving the system's no-automatic-trading boundary

## Concept Map

| # | Concept | Prerequisites | Status | Score |
|---|---------|---------------|--------|-------|
| 1 | 收益、亏损与最大回撤 | - | mastered | 80% |
| 2 | 股票、基金与指数 | 1 | mastered | 80% |
| 3 | 日线、成交量与复权价格 | 2 | mastered | 80% |
| 4 | 三张财务报表如何连接 | 2 | mastered | 80% |
| 5 | PE、PB、ROE 与企业质量 | 4 | mastered | 80% |
| 6 | 收益率、波动率、夏普与基准 | 1, 2 | mastered | 80% |
| 7 | 数据质量与未来数据偏差 | 3 | mastered | 80% |
| 8 | 股票池、因子、信号与策略 | 3, 5, 7 | mastered | 80% |
| 9 | 回测假设与结果可信度 | 1, 6, 7, 8 | mastered | 80% |
| 10 | 组合、分散与仓位风险 | 1, 6, 9 | mastered | 80% |
| 11 | 系统日常研究与人工决策流程 | 8, 9, 10 | mastered | 80% |

## Mastery Policy

- A concept advances only after the learner demonstrates at least 80% mastery through explanation and application.
- Diagnostic familiarity is not counted as mastery.
- The system is used for research, simulation, and record keeping, never automatic trade execution.

## Session Log

- [2026-07-26] New learning profile created in Chinese.
- [2026-07-26] Diagnosed as a beginner with useful market vocabulary and a strong system-building motivation.
- [2026-07-26] Concept 1 started at 50% after the annualized-return / maximum-drawdown diagnostic.
- [2026-07-26] Concept 1 increased to 63% after correctly identifying the peak-to-trough calculation; loss frequency remains the active gap.
- [2026-07-26] Concept 1 increased to 75% after correctly comparing observed loss-day frequency; synthesis of frequency and severity is next.
- [2026-07-26] Concept 1 remained at 75%: frequency and severity were distinguished, while an unsupported causal interpretation about market support and value recognition was identified.
- [2026-07-26] Concept 1 mastered at 80% after correctly separating a path-based metric from unsupported market and valuation causes.
- [2026-07-26] Concept 2 started at the 55% diagnostic baseline; index meaning is the first active gap.
- [2026-07-26] Concept 2 moved to 45% after a partial response: share-count weighting intuition was useful, but index points were still interpreted as a currency total.
- [2026-07-26] Concept 2 increased to 55% after recognizing that 1000 to 1050 points represents a 5% relative move, not 1050 yuan.
- [2026-07-26] Concept 2 increased to 65% after correctly identifying that larger-market-cap constituents have greater index influence.
- [2026-07-26] Concept 2 mastered at 80% after explaining index weighting and why an index move does not require every constituent to move in the same direction or amount.
- [2026-07-26] Concept 3 started at the 45% diagnostic baseline; raw bars are familiar, volume and corporate-action adjustments are next.
- [2026-07-26] Concept 3 increased to 60% after recognizing that a split can create a raw-price drop without the same proportional change in total holdings value.
- [2026-07-26] Concept 3 increased to 75% after calculating post-split total value correctly; adjusted price handling and volume remain.
- [2026-07-26] Concept 3 moved to 70%: volume intuition is useful, but participant count, intent, and manipulation cannot be inferred from volume alone.
- [2026-07-26] Concept 3 clarification round: learner recognized that both patterns record 100 shares; per-trade size, total traded value, and trade frequency are the remaining distinction.
- [2026-07-26] Concept 3 advanced to 78% after correctly holding price constant and showing equal total traded value; a concise volume definition was provided for teach-back.
- [2026-07-26] Concept 3 clarification: learner asked about the meaning of price-plus-volume changes and relative volume; next check will compare expansion and contraction against price direction.
- [2026-07-26] Concept 3 application: learner correctly identified that price up with 2x relative volume means more active turnover, while news and continuation are hypotheses rather than conclusions from volume alone.
- [2026-07-26] Concept 3 mastered at 80% after separating observed price/volume facts from causal explanations and predictions.
- [2026-07-26] Concept 4 started at the 30% diagnostic baseline; the first probe will connect profit and cash flow.
- [2026-07-26] Concept 4 increased to 45% after identifying possible cash pressure; next probe separates a business sale on credit from its cash collection.
- [2026-07-26] Concept 4 increased to 65% after correctly mapping a credit sale to profit, receivables, and delayed cash collection.
- [2026-07-26] Concept 4 increased to 75% after correctly mapping the later cash collection across all three statements.
- [2026-07-26] Concept 4 moved to 70% after a synthesis gap: a bad-debt recognition affects profit through an expense as well as reducing receivables; possible profit inflation was framed as a hypothesis needing corroboration.
- [2026-07-26] Concept 4 increased to 75% after proposing expansion spending and accumulating bad debt as competing hypotheses; evidence sources now need to be separated by statement.
- [2026-07-26] Concept 4 increased to 78% after adding industry and customer-base comparisons; final gap is translating those hypotheses into report fields and disclosures.
- [2026-07-26] Concept 4 remained at 78% after a field-mapping check; final synthesis will distinguish cash-flow evidence from disclosure evidence.
- [2026-07-26] Concept 4 moved to 75% after correct field mapping but an overstrong fraud conclusion; the final gate is distinguishing suspicion from proof.
- [2026-07-26] Learner deferred the fraud-verification branch. Keep the core three-statement relationships in scope; revisit forensic evidence only if it becomes useful for research decisions.
- [2026-07-26] Concept 4 mastered at 80% after correctly distinguishing borrowing, liabilities, assets, equity, and profit.
- [2026-07-26] Concept 5 started at the 20% diagnostic baseline; PE, PB, and ROE vocabulary is new.
- [2026-07-26] Concept 5 increased to 50% after identifying the higher market value as more expensive relative to equal current profit; profitability versus valuation remains the gap.
- [2026-07-26] Concept 5 increased to 60% after identifying reinvestment and market expansion as possible reasons for low current profit and a high valuation.
- [2026-07-26] Concept 5 increased to 70% after correctly calculating and explaining 10x and 50x PE valuations.
- [2026-07-26] Concept 5 increased to 75% after explaining 1x PB and identifying the lower-PB company as cheaper relative to net assets.
- [2026-07-26] Concept 5 remained at 75% after correct PB and ROE ranking but an arithmetic error in 100 divided by 500.
- [2026-07-26] Concept 5 increased to 78% after correctly converting ROE to 20% and 10%; final synthesis will separate low valuation from high quality.
- [2026-07-26] Concept 5 mastered at 80% after distinguishing cheap valuation from high capital efficiency and profit quality.
- [2026-07-26] Concept 6 started at the 30% diagnostic baseline; benchmark-relative performance is the first probe.
- [2026-07-26] Concept 6 increased to 50% after correctly identifying benchmark underperformance and possible market-driven returns.
- [2026-07-26] Concept 6 increased to 60% after correctly separating absolute loss from benchmark-relative outperformance.
- [2026-07-26] Concept 6 increased to 70% after correctly comparing equal-return paths by volatility and risk-adjusted quality.
- [2026-07-26] Concept 6 increased to 75% after calculating Sharpe values correctly; units and interpretation were tightened.
- [2026-07-26] Concept 6 mastered at 80% after correctly explaining compounded loss from sequential +50% and -50% returns.
- [2026-07-26] Concept 7 started at the 20% diagnostic baseline; information availability and lookahead bias are the first probe.
- [2026-07-26] Concept 7 increased to 50% after correctly identifying future-data leakage from a not-yet-published annual report.
- [2026-07-26] Concept 7 increased to 65% after correctly explaining survivorship bias from using today's constituent list for historical backtests.
- [2026-07-26] Concept 7 increased to 75% after correctly requiring point-in-time versions rather than later restated data.
- [2026-07-26] Concept 7 mastered at 80% after correctly using publication dates rather than report-period end dates for historical availability.
- [2026-07-26] Concept 8 started at the 30% diagnostic baseline; the first probe maps pool, factor, signal, and strategy roles.
- [2026-07-26] Concept 8 increased to 60% after correctly mapping pool, factors, signal, and holding rules in a Top-30 workflow.
- [2026-07-26] Concept 8 increased to 65% after identifying factor-driven signal changes; the remaining gap is rule definition versus resulting positions.
- [2026-07-26] Concept 8 increased to 75% after correctly separating static holding rules from dynamic signal membership and positions.
- [2026-07-26] Concept 8 remained at 75% after correctly identifying strategy-level capital and risk decisions but mislabeling an explicitly shared Top-30 signal as different.
- [2026-07-26] Concept 8 moved to 70% after identifying different final weights but missing that the weighting and cash rules themselves are also different.
- [2026-07-26] Concept 8 mastered at 80% after explaining that shared signals can produce different positions under different strategy rules.
- [2026-07-26] Concept 9 started at the 30% diagnostic baseline; execution and transaction-cost assumptions are the first probe.
- [2026-07-26] Concept 9 increased to 55% after identifying trade feasibility and missing transaction costs; same-close signal timing remains.
- [2026-07-26] Concept 9 increased to 65% after rejecting the impossible same-close fill and proposing post-signal execution prices.
- [2026-07-26] Concept 9 increased to 75% after rejecting impossible limit-up fills and requiring delayed execution plus signal and cost reassessment.
- [2026-07-26] Concept 9 clarification round: learner requested a smaller explanation of parameter overfitting; use a train-versus-test time split before the next mastery check.
- [2026-07-26] Concept 9 mastered at 80% after explaining why parameter selection and final out-of-sample testing must use separate periods.
- [2026-07-26] Concept 10 started at the 30% diagnostic baseline; concentration and diversification are the first probe.
- [2026-07-26] Concept 10 increased to 55% after correctly separating company or industry diversification from broad-market risk.
- [2026-07-26] Concept 10 increased to 65% after correctly preferring cross-industry diversification; correlation and offset are next.
- [2026-07-26] Concept 10 increased to 75% after correctly connecting lower correlation to risk offset and reduced shared-risk concentration.
- [2026-07-26] Concept 10 mastered at 80% after calculating a 21% portfolio loss contribution from a 70%-weighted position falling 30%.
- [2026-07-26] Concept 11 started at a 40% baseline based on the learner's system-use goal; the first probe will order the daily research workflow.
- [2026-07-26] Concept 11 increased to 60% after correctly ordering the daily workflow and identifying existing-position and risk review before manual action.
- [2026-07-27] Concept 11 increased to 75% after correctly handling an untradeable signal, industry concentration, information checks, and the manual-execution boundary.
- [2026-07-27] Concept 11 mastered at 80% after defining a complete decision record for later attribution and review.
- [2026-07-27] Session completed: all 11 concepts mastered at or above the 80% gate.
- [2026-07-27] Generated `quick-review.md`: formulas, dialogue-specific misconceptions, system workflow, decision checklist, record template, and 15 self-test questions.
- [2026-07-26] Concept 4 core synthesis: learner identified profit and two-sided cash movement in a buy-sell cycle; balance-sheet and equity links need one final check.
- [2026-07-26] Concept 4 advanced to 78% after introducing owners' equity as the residual after liabilities; next check separates borrowing from earnings.
