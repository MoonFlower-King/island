//! AI 消息、AI 工作台、批准请求、tokens 用量。

use super::*;

impl App {
    /// AI 工具发来的消息（`Island.exe --notify`）：更新工作台；做完了、等你确认的记下来并弹出提醒；
    /// 要你批准的弹出带按钮的胶囊，一直停着等你点。
    pub fn on_ai_messages(&mut self, msgs: Vec<forward::Inbound>) {
        use island::ai::Kind;
        let now = tick();
        let mut bench_changed = false;
        for m in msgs {
            let Some(event) = island::ai::parse(&m.source, &m.payload) else {
                if m.reply != 0 {
                    forward::answer(m.reply, forward::Answer::Terminal);
                }
                continue;
            };
            // 每次用工具都会来一条，不记日志
            if !matches!(event.kind, Kind::Started | Kind::Working) {
                log::info(&format!(
                    "AI 消息：{}（{}，{}）",
                    event.title(),
                    event.project,
                    if m.window != 0 {
                        "记下了窗口"
                    } else {
                        "没找到窗口"
                    }
                ));
            }
            bench_changed |= self.bench.apply(&event, now, m.window);
            let key = event.key();
            crate::win::usage::request(&key, &event.transcript);
            let target = if m.window != 0 {
                Target::Window(m.window)
            } else {
                Target::None
            };
            match event.kind {
                Kind::Approval if m.reply != 0 => {
                    self.next_approval += 1;
                    self.approvals.push(views::ApprovalItem {
                        id: self.next_approval,
                        key,
                        title: event.title(),
                        body: event.approval_body(),
                        reply: m.reply,
                        window: m.window,
                    });
                    if self.note_approval.is_none() {
                        self.show_approval();
                    }
                    self.ai_changed();
                }
                Kind::Done | Kind::Ask => {
                    if event.kind == Kind::Done {
                        // 这一轮做完了：还没答复的请求已经用不上了
                        self.drop_approvals(&key);
                    } else if self.approvals.iter().any(|a| a.key == key) {
                        // 已经在岛上问着了
                        continue;
                    }
                    let (title, body) = (event.title(), event.body());
                    self.add_ai(&title, &body, None, target.clone());
                    self.show_notification_for(views::GLYPH_AI, &title, &body, AI_NOTIFICATION_MS);
                    if self.note_approval.is_none() {
                        self.notification_target = target;
                    }
                }
                Kind::Ended => self.drop_approvals(&key),
                _ => {}
            }
        }
        if bench_changed && self.state == IslandState::Expanded && self.tab == Tab::Ai {
            self.redraw_expanded();
        }
    }

    /// 弹出排在最前的批准请求（带允许、拒绝按钮，一直停着）。
    pub(super) fn show_approval(&mut self) {
        let Some(a) = self.approvals.first().cloned() else {
            return;
        };
        self.raising_approval = true;
        self.show_notification_with(
            views::GLYPH_AI,
            None,
            &a.title,
            &a.body,
            APPROVAL_NOTIFICATION_MS,
        );
        self.raising_approval = false;
        self.notification_target = if a.window != 0 {
            Target::Window(a.window)
        } else {
            Target::None
        };
    }

    /// 答复第 `i` 个批准请求。"回终端处理"会顺便切到那个终端。
    pub fn decide(&mut self, i: usize, answer: forward::Answer) {
        if i >= self.approvals.len() {
            return;
        }
        let a = self.approvals.remove(i);
        log::info(&format!("批准请求：{}（{answer:?}）", a.title));
        forward::answer(a.reply, answer);
        if answer == forward::Answer::Terminal && a.window != 0 {
            jump::open(&Target::Window(a.window));
        }
        self.approvals_changed();
    }

    /// 同一个会话的请求已经用不上了（做完了、会话结束）：让等着的钩子退出，Claude Code 照常处理。
    pub(super) fn drop_approvals(&mut self, key: &str) {
        let before = self.approvals.len();
        self.approvals.retain(|a| {
            let keep = a.key != key;
            if !keep {
                forward::answer(a.reply, forward::Answer::Terminal);
            }
            keep
        });
        if self.approvals.len() != before {
            self.approvals_changed();
        }
    }

    /// 请求列表变了：弹着的那个没了就换下一个，或者收起。
    pub(super) fn approvals_changed(&mut self) {
        self.update_tray();
        if self.note_approval.is_some()
            && self.note_approval != self.approvals.first().map(|a| a.id)
        {
            if self.approvals.is_empty() {
                self.dismiss_notification();
            } else {
                self.show_approval();
            }
        }
        if self.state == IslandState::Expanded {
            self.redraw_expanded();
        }
    }

    /// 记一条 AI 消息（没看过），刷新所有显示"有新消息"的地方。
    pub(super) fn add_ai(
        &mut self,
        title: &str,
        body: &str,
        icon: Option<std::sync::Arc<crate::win::media::Art>>,
        target: Target,
    ) {
        let t = unsafe { GetLocalTime() };
        self.ai.insert(
            0,
            views::AiItem {
                title: title.into(),
                body: body.into(),
                at: format!("{:02}:{:02}", t.wHour, t.wMinute),
                read: false,
                icon,
                target,
                received: tick(),
            },
        );
        self.ai.truncate(50);
        self.ai_changed();
    }

    pub fn ai_unread(&self) -> bool {
        self.ai.iter().any(|m| !m.read)
    }

    pub(super) fn ai_changed(&mut self) {
        self.update_tray();
        self.redraw_clock();
        self.redraw_compact();
        self.update_line_tint();
        if self.state == IslandState::Expanded {
            self.redraw_expanded();
        }
    }

    pub(super) fn mark_ai_read(&mut self) {
        if self.ai_unread() {
            self.ai.iter_mut().for_each(|m| m.read = true);
            self.ai_changed();
        }
    }

    /// 后台数好了 tokens。
    pub fn on_usage(&mut self, results: Vec<(String, u64, u64)>) {
        let mut changed = false;
        for (key, total, today) in results {
            if let Some(a) = self.bench.agents.iter_mut().find(|a| a.key == key) {
                changed |= a.tokens != total;
                a.tokens = total;
            }
            changed |= self.tokens_today.insert(key, today) != Some(today);
        }
        if changed && self.state == IslandState::Expanded && self.tab == Tab::Ai {
            self.redraw_expanded();
        }
    }
}
