//! 工程文件的写盘那一半：保存、另存为、未保存确认。
//!
//! 自动保存（`.autosave`）的计时与崩溃恢复在 `app/project_autosave.rs`；这里只管
//! 「用户按下保存」这条路径，并在存完之后顺手把那份自动保存删掉。
//!
//! 工程入口（新建 / 打开 / 关闭）在 `timeline_view.rs`，这里只补上写盘与拦截：
//! 任何会丢掉当前工程的动作用 [`PlayerApp::request_project_action`] 走一遍，
//! 有未保存改动就先让用户选「保存 / 不保存 / 取消」。
//!
//! 确认框是自绘模态（[`ProjectConfirm`]），和播放器里别的对话框同一套外观：
//! 系统消息框只能给出「是 / 否 / 取消」，说不清「继续之后到底会发生什么」；
//! 而且它盖在时间线、播放列表上面时，跟窗口其他地方本来就是两种皮肤。

use std::path::Path;

use super::*;
use crate::timeline::PROJECT_EXTENSION;

/// 未保存确认框上的三个回答。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProjectConfirmChoice {
    Save,
    Discard,
    Cancel,
}

/// 正在等用户回答的未保存确认。
///
/// 回答之后要执行的那个动作也挂在这里：关掉对话框（Esc / × / 取消）就等于把它丢掉，
/// 省得在 `PlayerApp` 上再留一个「待办」字段，那两处迟早会对不上。
#[derive(Clone, Debug)]
pub(super) struct ProjectConfirm {
    action: AfterSaveConfirm,
}

impl AfterSaveConfirm {
    /// 确认框里那句「继续之后会发生什么」。
    fn describe(&self) -> String {
        match self {
            AfterSaveConfirm::NewProject => "继续会新建一个空工程".to_string(),
            AfterSaveConfirm::CloseProject => "继续会关闭当前工程".to_string(),
            AfterSaveConfirm::OpenProject(path) => format!("继续会打开「{}」", file_name(path)),
            AfterSaveConfirm::Quit => "继续会退出播放器".to_string(),
        }
    }
}

impl PlayerApp {
    /// 有没有未保存改动。
    pub(super) fn project_modified(&self) -> bool {
        match (&self.project, &self.project_saved) {
            (Some(project), Some(saved)) => !project.same_content(saved),
            // 有工程却没有基准，说明它压根没落过盘。
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// 保存到当前路径；从没存过就转成「另存为」。
    ///
    /// 返回 `false` 表示程序已经在退出的路上，调用方别再碰 `cx`。
    pub(super) fn save_project(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
        then: Option<AfterSaveConfirm>,
    ) -> bool {
        let Some(path) = self.project_path.clone() else {
            self.save_project_as(window, cx, then);
            return true;
        };
        if self.write_project(&path) {
            let alive = self.continue_after(then, cx);
            if alive {
                cx.notify();
            }
            return alive;
        }
        // 存盘失败：不再继续后面的动作，否则等于替用户选了「不保存」。
        cx.notify();
        true
    }

    /// 换一个路径保存。对话框在后台线程里阻塞，结果走 oneshot 回来。
    pub(super) fn save_project_as(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
        then: Option<AfterSaveConfirm>,
    ) {
        if self.dialog_open {
            return;
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let default_name = default_project_file_name(&project.sequence().name);
        // 优先落在工程旁边；新建的工程还没有位置，就沿用上次打开文件的目录。
        let directory = self
            .project_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .or_else(|| self.settings.last_dir.clone());
        self.dialog_open = true;

        let mut picker = rfd::FileDialog::new()
            .add_filter("工程文件", formats::PROJECT)
            .set_file_name(&default_name);
        if let Some(directory) = directory {
            picker = picker.set_directory(directory);
        }
        if let Some(window) = window {
            picker = picker.set_parent(window);
        }

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.save_file());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                let alive = match picked {
                    Some(path) => {
                        let path = with_project_extension(path);
                        this.remember_dir(&path);
                        if this.write_project(&path) {
                            this.continue_after(then, cx)
                        } else {
                            true
                        }
                    }
                    // 用户取消了对话框：后面的动作也一并取消。
                    None => true,
                };
                if alive {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 做一件会丢掉当前工程的事之前先问一句。
    ///
    /// 没有工程、或者没有未保存改动时直接执行，不打扰。
    pub(super) fn request_project_action(
        &mut self,
        action: AfterSaveConfirm,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || !self.project_modified() {
            self.run_project_action(action, cx);
            return;
        }
        // 原生文件选择框正在等结果、或者确认框已经开着：不叠第二个。
        if self.project_confirm.is_some() || self.dialog_open {
            return;
        }
        // 悬浮菜单跟着收掉：确认框一出来，菜单栏上那个「点过了」的高亮没必要再留着。
        self.menu = None;
        self.menu_bar = None;
        self.project_confirm = Some(ProjectConfirm { action });
        self.poke();
        cx.notify();
    }

    /// 回答确认框。
    ///
    /// 「保存」走的还是原来那条路：没落过盘的工程会先转成另存为，用户在文件选择框里
    /// 取消就等于回答「取消」，后面的动作不再执行。
    pub(super) fn resolve_project_confirm(
        &mut self,
        choice: ProjectConfirmChoice,
        cx: &mut Context<Self>,
    ) {
        let Some(confirm) = self.project_confirm.take() else {
            return;
        };
        let alive = match choice {
            ProjectConfirmChoice::Save => self.save_project(None, cx, Some(confirm.action)),
            ProjectConfirmChoice::Discard => {
                // 用户明确说了不保存：那份 `.autosave` 也不该再被捡回来。
                // `Drop` 只在「没有未保存改动」时才清理，这里必须补这一刀。
                self.clear_autosave();
                self.run_project_action(confirm.action, cx)
            }
            ProjectConfirmChoice::Cancel => true,
        };
        // 退出这条路最后会 `cx.quit()`，那之后不能再碰 `cx`。
        if alive {
            cx.notify();
        }
    }

    /// 确认框打开时独占键盘：Esc 取消，其余按键一律吃掉。
    ///
    /// 它盖在画面上，空格、左右这些播放器快捷键不该再传下去切播放或跳转；
    /// 同时它排在所有别的对话框按键处理之前，Esc 关掉的永远是这一层。
    pub(super) fn on_project_confirm_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.project_confirm.is_none() {
            return false;
        }
        if event.keystroke.key == "escape" {
            self.resolve_project_confirm(ProjectConfirmChoice::Cancel, cx);
        }
        true
    }

    /// 未保存确认的模态卡片。
    pub(super) fn render_project_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.project_confirm.as_ref()?;
        let consequence = confirm.action.describe();

        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .whitespace_normal()
                    .child("当前工程有未保存的改动。要保存之后再继续吗？"),
            )
            // 三个按钮各会带来什么后果，写在按钮上面：这句是用户按下去之前
            // 唯一能看到的说明。
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child(format!("选「不保存」，{consequence}。")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .child(settings_button(
                        "project-confirm-cancel",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.resolve_project_confirm(ProjectConfirmChoice::Cancel, cx);
                        }),
                    ))
                    .child(settings_button(
                        "project-confirm-discard",
                        "不保存",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.resolve_project_confirm(ProjectConfirmChoice::Discard, cx);
                        }),
                    ))
                    .child(primary_button(
                        "project-confirm-save",
                        "保存",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.resolve_project_confirm(ProjectConfirmChoice::Save, cx);
                        }),
                    )),
            )
            .into_any_element();

        Some(modal_card(
            "project-confirm",
            "工程还没有保存",
            body,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.resolve_project_confirm(ProjectConfirmChoice::Cancel, cx);
            }),
        ))
    }

    /// 真正执行一个「换工程」的动作。返回 `false` 表示程序正在退出。
    fn run_project_action(&mut self, action: AfterSaveConfirm, cx: &mut Context<Self>) -> bool {
        match action {
            AfterSaveConfirm::NewProject => {
                // 规格对话框要窗口才建得出输入框，这条路径上没有：记一笔，
                // 下一帧在 `render` 里打开，用户选完规格才真的建工程。
                self.pending_sequence_dialog = Some(SequencePurpose::New);
                true
            }
            AfterSaveConfirm::CloseProject => {
                self.close_project(cx);
                true
            }
            AfterSaveConfirm::OpenProject(path) => {
                self.open_project_with_recovery(path, cx);
                true
            }
            AfterSaveConfirm::Quit => {
                cx.quit();
                false
            }
        }
    }

    fn continue_after(&mut self, then: Option<AfterSaveConfirm>, cx: &mut Context<Self>) -> bool {
        match then {
            Some(action) => self.run_project_action(action, cx),
            None => true,
        }
    }

    /// 落盘并刷新基准。返回是否成功。
    fn write_project(&mut self, path: &Path) -> bool {
        self.stamp_view_state();
        let outcome = match self.project.as_mut() {
            Some(project) => project.save(path),
            None => return false,
        };
        match outcome {
            Ok(()) => {
                self.project_path = Some(path.to_path_buf());
                self.remember_project(path);
                // `save` 会把素材路径改写成相对形式，基准必须在这之后取。
                self.project_saved = self.project.clone();
                // 刚写过盘，`.autosave` 就是旧的了：删掉并在下次编辑前不再写。
                self.autosave.mark_saved();
                autosave::clear(path);
                self.flash(format!("已保存工程「{}」", file_name(path)));
                true
            }
            Err(error) => {
                log_warn!("保存工程失败: {error}");
                self.error = Some(Banner::new(
                    "保存工程失败",
                    format!("{error}\n\n工程还在内存里，可以换个位置再存一次。"),
                ));
                false
            }
        }
    }

    /// 把当前的缩放与滚动写进工程。
    ///
    /// 正常保存和自动保存都要带上：从自动保存恢复之后，用户看到的还是崩溃前
    /// 那个位置。视图状态不在 `same_content` 的比较范围里，写进去不会伪造「未保存」。
    pub(super) fn stamp_view_state(&mut self) {
        let zoom = self.timeline.zoom();
        let scroll = self.timeline.scroll();
        if let Some(project) = self.project.as_mut() {
            project.ui.zoom = Some(zoom);
            project.ui.scroll = scroll;
        }
    }

    /// 记一笔「最近打开的工程」。
    ///
    /// 同一个路径只留一条并移到最前：反复打开同一个工程不该把整张表刷满。
    pub(super) fn remember_project(&mut self, path: &Path) {
        crate::settings::push_recent(&mut self.settings.recent_projects, path);
    }

    /// 离开当前工程时清掉它的 `.autosave`。
    ///
    /// 内容要么刚被保存、要么被用户主动丢弃，留着那份只会在下次打开时白问一句。
    pub(super) fn clear_autosave(&mut self) {
        if let Some(path) = &self.project_path {
            autosave::clear(path);
        }
        self.autosave.mark_saved();
    }
}

/// 把名字里的文件名非法字符换掉，并清掉结尾的空格与点。
///
/// Windows 不允许文件名以空格或点结尾；工程文件与导出文件共用这一套清洗。
pub(super) fn safe_file_stem(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| if r#"\:*?"<>|/"#.contains(ch) { '_' } else { ch })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.');
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned.to_string()
    }
}

/// 另存为对话框里的默认文件名：序列名去掉文件名非法字符，再加上扩展名。
fn default_project_file_name(sequence_name: &str) -> String {
    format!(
        "{}.{PROJECT_EXTENSION}",
        safe_file_stem(sequence_name, "工程")
    )
}

/// 用户在对话框里删掉了扩展名时补回去，别存出一个打不开的工程。
fn with_project_extension(path: PathBuf) -> PathBuf {
    if formats::is_project(&path) {
        return path;
    }
    let mut name = path.into_os_string();
    name.push(".");
    name.push(PROJECT_EXTENSION);
    PathBuf::from(name)
}

pub(super) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 另存为的默认文件名清掉非法字符() {
        assert_eq!(default_project_file_name("序列 1"), "序列 1.ngyproj");
        assert_eq!(default_project_file_name("a/b:c*d?"), "a_b_c_d_.ngyproj");
        assert_eq!(default_project_file_name("   "), "工程.ngyproj");
        assert_eq!(default_project_file_name("尾巴...."), "尾巴.ngyproj");
    }

    #[test]
    fn 保存路径缺扩展名时补上() {
        assert_eq!(
            with_project_extension(PathBuf::from("demo")),
            PathBuf::from("demo.ngyproj")
        );
        assert_eq!(
            with_project_extension(PathBuf::from("demo.ngyproj")),
            PathBuf::from("demo.ngyproj")
        );
        // 别的扩展名不是「已经带了」，要接在后面而不是替换。
        assert_eq!(
            with_project_extension(PathBuf::from("demo.txt")),
            PathBuf::from("demo.txt.ngyproj")
        );
    }

    #[test]
    fn 确认框写清楚继续之后会发生什么() {
        assert_eq!(
            AfterSaveConfirm::OpenProject(PathBuf::from("E:/片库/demo.ngyproj")).describe(),
            "继续会打开「demo.ngyproj」"
        );
        // 三条不带参数的路径各有一句话，别退化成空串。
        for action in [
            AfterSaveConfirm::NewProject,
            AfterSaveConfirm::CloseProject,
            AfterSaveConfirm::Quit,
        ] {
            assert!(action.describe().starts_with("继续会"));
        }
    }
}
