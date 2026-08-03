//! 三个角色的 prompt 构造。
//!
//! 每个角色跑在**独立的 CLI thread** 上（设计文档 §3：角色间用产物交接、
//! 不共享上下文，评审要独立视角）。因此每段 prompt 必须自包含——
//! 拿到它的模型不知道任务目标，也看不到上一个角色的对话。

use super::Verdict;

/// prompt 构造入参。字段多，用结构体避免位置参数。
#[derive(Debug, Clone)]
pub struct RolePromptInput<'a> {
    /// 用户原始诉求（team_tasks.goal）
    pub goal: &'a str,
    /// 闸门第一轮产出的四段分析（team_tasks.analysis）。可能为空。
    pub analysis: Option<&'a str>,
    /// trace_code / quant_code / general_task，用于追加项目特定约束
    pub agent_kind: &'a str,
    /// 本角色第几轮（开发被打回后是 2、3……）
    pub round: i32,
    /// 上游角色的交接产物。开发首轮为 None；
    /// 评审看开发的、测试看评审的、打回后的开发看评审的。
    pub upstream: Option<UpstreamHandoff<'a>>,
}

/// 上游角色交接给本角色的产物（来自 team_task_runs 的 handoff / summary 列）。
#[derive(Debug, Clone)]
pub struct UpstreamHandoff<'a> {
    /// 上游角色 id，如 "developer"（看板/日志用；当前 prompt 正文未引用）
    #[allow(dead_code)]
    pub role: &'a str,
    pub summary: Option<&'a str>,
    /// 上游 verdict（看板用；prompt 侧主要看 summary/blocking）
    #[allow(dead_code)]
    pub verdict: Option<Verdict>,
    pub blocking: Option<&'a str>,
    pub changed_files: Option<i32>,
}

// ---------------------------------------------------------------------------
// 公共段落
// ---------------------------------------------------------------------------

/// 工作区与安全边界。所有角色共用。
///
/// 措辞对齐 `cli_agent::local_agent_prompt`，**不要**出现 `/opt/hank` 之类
/// server 绝对路径——client-only 会话跑在用户本机，泄露 server 路径会让模型
/// 去找不存在的目录。
fn workspace_constraints(agent_kind: &str) -> String {
    let mut s = String::from(
        "\n\n运行约束：\n\
         - 只操作 hank-cli 提供的当前工作目录及其子目录。\n\
         - 遵循目录中的 AGENTS.md / CLAUDE.md 等项目规则。\n\
         - 不要读取或修改凭据、密钥或本机 Agent 认证配置。\n",
    );
    if agent_kind == "quant_code" {
        s.push_str("- 修改 quant 前必须读取 quant/AGENTS.md，并遵守禁止交易能力的产品边界。\n");
    }
    s
}

/// 任务背景：目标 + 第一轮分析。每个角色的 prompt 都以它开头。
///
/// analysis 为 None 或空白时整段省略——不要留一个空的「先前的只读分析」标题，
/// 模型会以为分析丢了。
fn task_context(input: &RolePromptInput<'_>) -> String {
    let mut s = format!("## 任务目标\n{}\n", input.goal);
    if let Some(analysis) = input.analysis {
        let trimmed = analysis.trim();
        if !trimmed.is_empty() {
            s.push_str("\n## 先前的只读分析\n");
            s.push_str(trimmed);
            s.push('\n');
        }
    }
    s
}

/// 交接段要求。needs_verdict 为 false 的角色（开发）不要求 verdict 行。
///
/// 格式必须与 `super::parse_handoff` 严格对齐：标题含「交接」、键名
/// verdict/changed_files/summary/blocking、半角或全角冒号。写错会导致
/// 评审/测试的 verdict 变成 Unknown，任务直接 failed。
///
/// 说明段用 `## 输出格式要求` 而不是 `## 交接`：若说明段也叫「交接」，
/// 模型常把两段一起抄回，回复里出现两个 `## 交接`；即便解析器已改为
/// 取最后一个，也不该主动在 prompt 里埋双标题。
///
/// 模板值用尖括号占位（如 `<纯数字>`）而不是「本轮改动的文件数」这类
/// 像答案的自然语言——模型若整段照抄，`changed_files` 解析成 None、
/// `Verdict::parse` 判 Unknown，**这是期望行为**：没真的填值就该失败，
/// 而不是被当成有效结论。
fn handoff_requirement(needs_verdict: bool) -> String {
    let mut s = String::from(
        "\n\n## 输出格式要求\n\
         在回复的**最后**输出下面这段，键名与格式不要改动：\n\
         \n\
         ## 交接\n",
    );
    if needs_verdict {
        // verdict 只允许 pass / reject 两个值；写别的（如「基本通过」）会被
        // Verdict::parse 判成 Unknown，按状态机规则任务直接 failed。
        s.push_str("verdict: <pass 或 reject，二选一，不要写其他词>\n");
    }
    s.push_str(
        "changed_files: <纯数字>\n\
         summary: <一句话说明判定理由>\n\
         blocking: <阻塞项；没有就写 none>\n",
    );
    s
}

// ---------------------------------------------------------------------------
// 三个角色
// ---------------------------------------------------------------------------

/// 开发角色：按分析执行改动。
///
/// 每个角色跑在独立 CLI thread 上，看不到上一轮对话——所以 prompt 必须自包含。
/// round > 1 表示被评审打回，此时必须把评审意见注入——否则模型在新 thread 上
/// 看不到自己上一轮做了什么，也不知道为什么被打回，会从头再来一遍。
pub fn developer_prompt(input: &RolePromptInput<'_>) -> String {
    let mut s = String::from("【本轮角色：开发】\n\n");
    s.push_str(&task_context(input));

    if input.round > 1 {
        s.push_str("\n## 上一轮被评审打回\n");
        s.push_str("请针对下面的打回意见修改，不要重做无关部分。\n");
        if let Some(up) = &input.upstream {
            if let Some(summary) = up.summary {
                let t = summary.trim();
                if !t.is_empty() {
                    s.push_str("\n评审意见：");
                    s.push_str(t);
                    s.push('\n');
                }
            }
            if let Some(blocking) = up.blocking {
                let t = blocking.trim();
                if !t.is_empty() {
                    s.push_str("阻塞项：");
                    s.push_str(t);
                    s.push('\n');
                }
            }
        }
    } else {
        s.push_str("\n本轮请按上面的分析执行代码修改。\n");
    }

    s.push_str("\n完成后请自行验证（编译 / 跑与改动相关的测试），确认无回归再结束。\n");
    s.push_str(&workspace_constraints(input.agent_kind));
    s.push_str(&handoff_requirement(false));
    s
}

/// 评审角色：独立审查开发产出。
///
/// 刻意不 resume 开发的 thread，所以这里要告诉它自己去看 diff——
/// 它拿到的是一个干净 thread，只有本 prompt 里的信息。
///
/// 【本轮只读】约束：评审不要改代码。理由与闸门第一轮相同——
/// CLI 以 bypass-approvals 启动，沙箱不会拦写操作，只能靠指令约束。
pub fn reviewer_prompt(input: &RolePromptInput<'_>) -> String {
    let mut s = String::from("【本轮角色：评审】\n\n");
    s.push_str(&task_context(input));

    s.push_str("\n## 开发的自述\n");
    match &input.upstream {
        Some(up) => {
            match up.summary {
                Some(summary) if !summary.trim().is_empty() => {
                    s.push_str(summary.trim());
                    s.push('\n');
                }
                _ => {
                    s.push_str("（开发未留下 summary）\n");
                }
            }
            if let Some(n) = up.changed_files {
                s.push_str(&format!("开发声称改动文件数：{n}\n"));
            }
        }
        None => {
            s.push_str("上一轮没有留下交接说明，请完全依据 diff 判断。\n");
        }
    }

    s.push_str(
        "\n## 评审要求\n\
         1. **先用 `git diff`（必要时 `git status`）看清本轮实际改动**，再对照任务目标判断。\n\
         开发的自述可能与实际改动不符，**以 diff 为准**。\n\
         2. 判定口径——出现以下任一情况应写 `reject`：\n\
         - 偏离任务目标\n\
         - 引入明显缺陷（逻辑错误、崩溃路径、安全问题）\n\
         - 改了任务范围外的文件\n\
         - 破坏既有行为\n\
         其余情况可写 `pass`。\n\
         \n\
         【本轮只读】**不要改代码**。可以读文件、跑只读检查、看 git，\
         但不要修改、创建、删除任何文件，也不要执行会改变状态的命令。\n",
    );

    s.push_str(&workspace_constraints(input.agent_kind));
    s.push_str(&handoff_requirement(true));
    s
}

/// 测试角色：跑测试并验证行为。
///
/// 每个角色跑在独立 CLI thread 上，必须自包含注入目标与上游结论。
pub fn tester_prompt(input: &RolePromptInput<'_>) -> String {
    let mut s = String::from("【本轮角色：测试】\n\n");
    s.push_str(&task_context(input));

    s.push_str("\n## 评审结论\n");
    match &input.upstream {
        Some(up) => {
            if let Some(summary) = up.summary {
                let t = summary.trim();
                if !t.is_empty() {
                    s.push_str(t);
                    s.push('\n');
                } else {
                    s.push_str("（评审未留下 summary）\n");
                }
            } else {
                s.push_str("（评审未留下 summary）\n");
            }
        }
        None => {
            s.push_str("（无上游评审交接，请按任务目标自行验证）\n");
        }
    }

    s.push_str(
        "\n## 测试要求\n\
         1. 按项目约定跑测试：先读 CLAUDE.md / AGENTS.md 找命令，\
         **不要凭猜测编命令**。优先跑与本轮改动相关的测试矩阵。\n\
         2. 判定口径：\n\
         - 测试失败，或改动没有达到任务目标 → `reject`\n\
         - **只有测试确实通过才写 `pass`**\n\
         3. 允许写测试文件（补测例、修 flaky fixture 等），但**不要为了让测试通过而改业务代码**——\
         那是开发的职责；测试角色改业务代码会让「测试通过」失去意义。\n",
    );

    s.push_str(&workspace_constraints(input.agent_kind));
    s.push_str(&handoff_requirement(true));
    s
}

// ---------------------------------------------------------------------------
// 单测
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team_task::{parse_handoff, role_prompt, Verdict};

    fn base_input(goal: &'static str) -> RolePromptInput<'static> {
        RolePromptInput {
            goal,
            analysis: Some("## 目标\n修登录超时\n\n## 范围\nauth 模块"),
            agent_kind: "trace_code",
            round: 1,
            upstream: None,
        }
    }

    // ----- 5.1 自包含性 -----

    #[test]
    fn developer_prompt_is_self_contained() {
        let input = base_input("修复登录超时问题");
        let p = developer_prompt(&input);
        assert!(p.contains("修复登录超时问题"));
        assert!(p.contains("修登录超时"));
        assert!(p.contains("auth 模块"));
        assert!(p.contains("【本轮角色：开发】"));
    }

    #[test]
    fn reviewer_prompt_is_self_contained() {
        let mut input = base_input("修复登录超时问题");
        input.upstream = Some(UpstreamHandoff {
            role: "developer",
            summary: Some("改了 auth 超时时间"),
            verdict: None,
            blocking: None,
            changed_files: Some(2),
        });
        let p = reviewer_prompt(&input);
        assert!(p.contains("修复登录超时问题"));
        assert!(p.contains("修登录超时"));
        assert!(p.contains("【本轮角色：评审】"));
    }

    #[test]
    fn tester_prompt_is_self_contained() {
        let mut input = base_input("修复登录超时问题");
        input.upstream = Some(UpstreamHandoff {
            role: "reviewer",
            summary: Some("代码质量可接受"),
            verdict: Some(Verdict::Pass),
            blocking: None,
            changed_files: Some(0),
        });
        let p = tester_prompt(&input);
        assert!(p.contains("修复登录超时问题"));
        assert!(p.contains("修登录超时"));
        assert!(p.contains("【本轮角色：测试】"));
    }

    // ----- 5.2 交接段与 parse_handoff 往返一致 -----

    #[test]
    fn developer_prompt_handoff_roundtrip_no_verdict() {
        let p = developer_prompt(&base_input("做个小改动"));
        // 开发角色不要求 verdict 行
        assert!(!p.contains("verdict:"));
        assert!(p.contains("## 交接"));
        assert!(p.contains("changed_files:"));
        assert!(p.contains("summary:"));
        assert!(p.contains("blocking:"));

        // 按 prompt 里给的格式手写一段模型可能的回复
        let model_reply = "\
改完了，编译通过。

## 交接
changed_files: 3
summary: 调整了超时与重试逻辑
blocking: none
";
        let h = parse_handoff(model_reply);
        assert_eq!(h.changed_files, Some(3));
        assert_eq!(h.summary.as_deref(), Some("调整了超时与重试逻辑"));
        assert_eq!(h.blocking, None);
        assert_eq!(h.verdict, None);
    }

    #[test]
    fn reviewer_prompt_handoff_roundtrip_with_verdict() {
        let mut input = base_input("评审目标");
        input.upstream = Some(UpstreamHandoff {
            role: "developer",
            summary: Some("已改"),
            verdict: None,
            blocking: None,
            changed_files: Some(1),
        });
        let p = reviewer_prompt(&input);
        assert!(p.contains("verdict: <pass 或 reject"));

        let model_reply = "\
看了 diff，可以接受。

## 交接
verdict: pass
changed_files: 0
summary: 改动符合目标，无明显缺陷
blocking: none
";
        let h = parse_handoff(model_reply);
        assert_eq!(h.verdict, Some(Verdict::Pass));
        assert_eq!(h.changed_files, Some(0));
        assert_eq!(h.summary.as_deref(), Some("改动符合目标，无明显缺陷"));
        assert_eq!(h.blocking, None);
    }

    #[test]
    fn tester_prompt_handoff_roundtrip_reject() {
        let mut input = base_input("测试目标");
        input.upstream = Some(UpstreamHandoff {
            role: "reviewer",
            summary: Some("评审通过"),
            verdict: Some(Verdict::Pass),
            blocking: None,
            changed_files: Some(0),
        });
        let p = tester_prompt(&input);
        assert!(p.contains("verdict: <pass 或 reject"));

        let model_reply = "\
相关单测失败。

## 交接
verdict: reject
changed_files: 1
summary: 登录超时相关测试未通过
blocking: auth_test 失败
";
        let h = parse_handoff(model_reply);
        assert_eq!(h.verdict, Some(Verdict::Reject));
        assert_eq!(h.changed_files, Some(1));
        assert_eq!(h.summary.as_deref(), Some("登录超时相关测试未通过"));
        assert_eq!(h.blocking.as_deref(), Some("auth_test 失败"));
    }

    // ----- 5.3 打回轮次带评审意见 -----

    #[test]
    fn developer_round2_includes_review_feedback() {
        let input = RolePromptInput {
            goal: "修复登录超时",
            analysis: Some("分析内容"),
            agent_kind: "trace_code",
            round: 2,
            upstream: Some(UpstreamHandoff {
                role: "reviewer",
                summary: Some("漏了错误处理"),
                verdict: Some(Verdict::Reject),
                blocking: Some("缺单测"),
                changed_files: Some(0),
            }),
        };
        let p = developer_prompt(&input);
        assert!(p.contains("打回"), "应出现打回字样: {p}");
        assert!(p.contains("漏了错误处理"));
        assert!(p.contains("缺单测"));
        assert!(p.contains("不要重做无关部分"));
    }

    // ----- 5.4 upstream = None 时的降级 -----

    #[test]
    fn reviewer_without_upstream_still_usable() {
        let input = base_input("无上游的评审");
        let p = reviewer_prompt(&input);
        assert!(!p.contains("None"));
        assert!(
            p.contains("依据 diff") || p.contains("以 diff 为准"),
            "应有 diff 兜底措辞: {p}"
        );
        assert!(p.contains("【本轮角色：评审】"));
        assert!(p.contains("## 交接"));
    }

    // ----- 5.5 analysis = None 时不留空标题 -----

    #[test]
    fn analysis_none_omits_analysis_heading() {
        let input = RolePromptInput {
            goal: "随便做点事",
            analysis: None,
            agent_kind: "trace_code",
            round: 1,
            upstream: None,
        };
        let p = developer_prompt(&input);
        assert!(!p.contains("先前的只读分析"));
        assert!(p.contains("## 任务目标"));
        assert!(p.contains("随便做点事"));
    }

    #[test]
    fn analysis_blank_omits_analysis_heading() {
        let input = RolePromptInput {
            goal: "目标",
            analysis: Some("   \n  "),
            agent_kind: "trace_code",
            round: 1,
            upstream: None,
        };
        let p = reviewer_prompt(&input);
        assert!(!p.contains("先前的只读分析"));
    }

    // ----- 5.6 工作区边界 -----

    #[test]
    fn prompts_do_not_embed_server_workspace_paths() {
        let input = base_input("修 bug");
        for (name, p) in [
            ("developer", developer_prompt(&input)),
            ("reviewer", reviewer_prompt(&input)),
            ("tester", tester_prompt(&input)),
        ] {
            assert!(p.contains("hank-cli"), "{name} 应含 hank-cli");
            assert!(!p.contains("/opt/hank"), "{name} 不应含 /opt/hank");
            assert!(!p.contains("/workspace"), "{name} 不应含 /workspace");
        }
    }

    // ----- 5.7 quant_code 追加约束 -----

    #[test]
    fn quant_code_adds_agents_md_constraint() {
        let mut input = base_input("改 quant 指标");
        input.agent_kind = "quant_code";
        let p = developer_prompt(&input);
        assert!(p.contains("quant/AGENTS.md"));

        input.agent_kind = "trace_code";
        let p = developer_prompt(&input);
        assert!(!p.contains("quant/AGENTS.md"));
    }

    // ----- 5.8 评审只读约束 -----

    #[test]
    fn reviewer_is_read_only_developer_is_not() {
        let input = base_input("任务");
        let rev = reviewer_prompt(&input);
        assert!(
            rev.contains("不要改代码") || rev.contains("**不要改代码**"),
            "评审应含只读约束: {rev}"
        );

        let dev = developer_prompt(&input);
        assert!(
            !dev.contains("不要改代码") && !dev.contains("**不要改代码**"),
            "开发不应含只读约束: {dev}"
        );
    }

    // ----- 5.9 role_prompt 查表 -----

    #[test]
    fn role_prompt_lookup() {
        let input = base_input("查表测试");
        assert!(role_prompt("developer", &input).is_some());
        assert!(role_prompt("reviewer", &input).is_some());
        assert!(role_prompt("tester", &input).is_some());
        assert!(role_prompt("docs", &input).is_none());
        assert!(role_prompt("", &input).is_none());

        let p = role_prompt("developer", &input).unwrap();
        assert!(p.contains("【本轮角色：开发】"));
        assert!(p.contains("查表测试"));
    }

    /// prompt 里只能有一个 `## 交接`（模板段），说明段用 `## 输出格式要求`，
    /// 避免自己制造双标题被模型回声后抢先匹配。
    #[test]
    fn prompt_has_single_handoff_header() {
        let input = base_input("单标题检查");
        for (name, p) in [
            ("developer", developer_prompt(&input)),
            ("reviewer", reviewer_prompt(&input)),
            ("tester", tester_prompt(&input)),
        ] {
            let n = p.matches("## 交接").count();
            assert_eq!(n, 1, "{name} 应恰好一个 ## 交接，实际 {n}：\n{p}");
            assert!(
                p.contains("## 输出格式要求"),
                "{name} 说明段应使用 ## 输出格式要求"
            );
        }
    }
}
