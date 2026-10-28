//! 文件关联：设为默认、维护开关、撤销与状态刷新。

use super::*;

impl PlayerApp {
    /// 用户主动点了「设为默认」，所以成败都要说清楚——和启动时的静默自愈不同。
    pub(super) fn set_default_player(&mut self) {
        if !platform::associations::supported() {
            self.flash(format!(
                "{} 上还没有实现自动关联，请手动在系统设置里指定默认播放器",
                std::env::consts::OS
            ));
            return;
        }

        match platform::associations::register() {
            Ok(status) => {
                self.refresh_association_status();
                let mut lines = Vec::new();
                if let Ok(exe) = std::env::current_exe()
                    && is_development_build(&exe)
                {
                    lines.push(format!("注意：这是开发构建，关联指向 {}", exe.display()));
                    lines.push("cargo clean 之后关联会失效。".to_string());
                }
                lines.push(format!("已登记 {} 个格式。", status.registered));

                // Windows 上还得用户去设置页点一下；另两个平台注册即生效。
                match platform::associations::open_system_settings() {
                    Ok(()) => lines.push(platform::associations::guidance().to_string()),
                    Err(_) => lines.push("请打开 设置 > 应用 > 默认应用 选择本程序。".to_string()),
                }
                if !self.settings.maintain_associations {
                    lines.push("自动维护已关闭，本次只注册一次。".to_string());
                }
                self.flash(lines.join("\n"));
            }
            Err(error) => self.report_association_failure("注册文件关联失败", &error),
        }
    }

    pub(super) fn open_default_apps(&mut self) {
        match platform::associations::open_system_settings() {
            Ok(()) => self.flash("已打开系统的「默认应用」设置"),
            Err(error) => self.report_association_failure("打开系统设置失败", &error),
        }
    }

    pub(super) fn toggle_maintain_associations(&mut self) {
        self.settings.maintain_associations = !self.settings.maintain_associations;
        // 这个开关决定的是「下次启动」的行为，等到窗口关闭再落盘可能就等不到了。
        self.settings.save();
        if self.settings.maintain_associations {
            self.flash("已开启：每次启动重新登记文件关联");
        } else {
            self.flash("已关闭：启动时不再改动文件关联");
        }
    }

    pub(super) fn remove_associations(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let description = format!(
            "将删除本程序向系统登记的全部文件关联（{} 个格式）。\n\n\
             系统里已经选定的默认播放器不受影响——要改回来，请到 设置 > 应用 > 默认应用 手动调整。",
            platform::associations::count()
        );
        let dialog = rfd::MessageDialog::new()
            .set_parent(window)
            .set_title("移除文件关联")
            .set_description(description)
            .set_buttons(rfd::MessageButtons::YesNo);

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.show() == rfd::MessageDialogResult::Yes);
        });

        cx.spawn(async move |this, cx| {
            let confirmed = rx.await.unwrap_or(false);
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if confirmed {
                    match platform::associations::remove() {
                        Ok(()) => {
                            this.refresh_association_status();
                            this.flash("已移除本程序的文件关联");
                        }
                        Err(error) => this.report_association_failure("移除文件关联失败", &error),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 自动维护：把注册项按当前 exe 的路径重写一遍。
    ///
    /// 只在开关打开、平台支持、并且注册项确实过期时才动手——已经指对了就什么都不做。
    /// 放后台线程：启动路径上不该花时间等注册表。
    pub(super) fn auto_maintain_associations(&self) {
        if !self.settings.maintain_associations || !platform::associations::supported() {
            return;
        }
        std::thread::spawn(|| {
            let status = platform::associations::status();
            let up_to_date = status.registered == platform::associations::count()
                && status
                    .target
                    .as_ref()
                    .is_some_and(|target| association_target_matches(target));
            if up_to_date {
                return;
            }
            // 启动时的失败不打扰用户：注册表被策略锁死时每次开机糊一脸错更烦人。
            if let Err(error) = platform::associations::register() {
                log_warn!("自动维护文件关联失败: {error:#}");
            }
        });
    }

    pub(super) fn refresh_association_status(&mut self) {
        self.association_status = platform::associations::status();
    }

    /// 关联失败统一走红色横幅：里面的手动操作路径一行字放不下。
    pub(super) fn report_association_failure(
        &mut self,
        title: &'static str,
        error: &anyhow::Error,
    ) {
        log_warn!("{title}: {error:#}");
        self.flash(title);
        self.error = Some(Banner::new(
            title,
            format!("{error:#}\n\n也可以在系统设置里手动指定默认播放器。"),
        ));
    }
}
