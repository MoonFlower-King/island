//! 翻译标签的状态：输入、译文、进度，以及"翻译服务设置"页正在改的草稿。

use island::settings::Settings;
use island::translate::{self, Format, Profile};

use super::net;

/// 设置页里正在改、还没保存的配置。
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub provider: String,
    pub format: Format,
    pub base_url: String,
    pub model: String,
    /// 新填的 Key；None 表示不改已保存的。
    pub key: Option<String>,
    pub has_stored_key: bool,
}

impl Draft {
    pub fn load(settings: &Settings, provider: &str) -> Self {
        let p = settings.translate_profile(provider);
        Self {
            provider: translate::preset(provider).id.to_string(),
            format: p.format,
            base_url: p.base_url,
            model: p.model,
            key: None,
            has_stored_key: p.protected_key.is_some(),
        }
    }

    /// 检查并写进设置（不负责保存到磁盘）。返回错误说明。
    pub fn apply(&self, settings: &mut Settings) -> Result<(), String> {
        if !translate::is_valid(&self.base_url) {
            return Err(translate::INVALID_URL_HINT.into());
        }
        if self.model.trim().is_empty() {
            return Err("请填写模型名称".into());
        }
        let mut profile = settings.translate_profile(&self.provider);
        profile.format = self.format;
        profile.base_url = self.base_url.trim().into();
        profile.model = self.model.trim().into();
        if let Some(key) = self.key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            profile.protected_key = net::protect(key);
            if profile.protected_key.is_none() {
                return Err("保存 Key 失败（系统加密不可用）".into());
            }
        }
        settings.set_translate_profile(&self.provider, &profile);
        settings.translate_provider = self.provider.clone();
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Busy,
    Error(String),
}

pub struct TranslateUi {
    pub input: String,
    pub output: String,
    pub status: Status,
    /// 当前请求编号（0 表示没有）。
    pub job: u64,
    /// 这次请求是设置页的"测试"。
    pub testing: bool,
    pub settings_open: bool,
    pub draft: Draft,
    /// 设置页底部的提示：(文字, 是不是错误)。None 时显示请求地址。
    pub hint: Option<(String, bool)>,
    /// 译文滚动位置（DIP）；None 表示跟着最新内容走。
    pub output_scroll: Option<f32>,
    next_job: u64,
}

impl TranslateUi {
    pub fn new(settings: &Settings) -> Self {
        Self {
            input: String::new(),
            output: String::new(),
            status: Status::Idle,
            job: 0,
            testing: false,
            settings_open: false,
            draft: Draft::load(settings, &settings.translate_provider),
            hint: None,
            output_scroll: None,
            next_job: 1,
        }
    }

    /// 当前服务的 Key：先用保存的，没有的话官方接口读环境变量。
    pub fn key_for(profile: &Profile) -> Option<String> {
        profile
            .protected_key
            .as_deref()
            .and_then(net::unprotect)
            .filter(|k| !k.trim().is_empty())
            .or_else(|| {
                translate::env_key_name(profile.format, &profile.base_url)
                    .and_then(|name| std::env::var(name).ok())
                    .filter(|k| !k.trim().is_empty())
            })
    }

    /// 界面上的简短说明，如"DeepSeek · deepseek-chat"。
    pub fn summary(settings: &Settings) -> String {
        let id = &settings.translate_provider;
        let p = settings.translate_profile(id);
        format!("{} · {}", translate::preset(id).name, p.model)
    }

    /// 准备一次请求。返回错误说明（第二个值表示要去设置里改）。
    pub fn begin(&mut self, settings: &Settings, text: &str) -> Result<net::Job, (String, bool)> {
        let text = text.trim();
        if text.is_empty() {
            return Err(("先输入要翻译的文字".into(), false));
        }
        let id = &settings.translate_provider;
        let p = settings.translate_profile(id);
        if p.model.trim().is_empty() {
            return Err(("请先填写模型名称".into(), true));
        }
        if !translate::is_valid(&p.base_url) {
            return Err((translate::INVALID_URL_HINT.into(), true));
        }
        let key = Self::key_for(&p);
        if key.is_none() && !translate::is_loopback(&p.base_url) {
            return Err((
                format!("请先填写 {} 的 API Key", translate::preset(id).name),
                true,
            ));
        }
        let job = self.next_job;
        self.next_job += 1;
        self.job = job;
        Ok(net::Job {
            id: job,
            format: p.format,
            url: translate::request_url(p.format, &p.base_url),
            key,
            body: translate::request_body(p.format, &p.base_url, &p.model, text),
            model: p.model,
        })
    }
}
