//! 显卡检测 GUI 弹窗（egui / eframe 0.36）
//!
//! 窗口结构：
//!   ┌──────────────────────────────────────┐
//!   │         显卡识别检测  (标题)           │
//!   │  1. 硬件枚举    ✅ NVIDIA RTX 3060   │
//!   │  2. 驱动检查    ✅ nvidia-smi 正常    │
//!   │  3. 稳定性采样  ⏳ 采样 2/3 ...       │
//!   │ ──────────────────────────────────── │
//!   │ 明细: 多行检测结果                    │
//!   │ 结果: PASS / FAIL（大字号着色）       │
//!   │ [ 确认 ] 或  自动关闭倒计时 Ns        │
//!   └──────────────────────────────────────┘

use eframe::egui::{self, Color32, RichText};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use crate::detect::{self, Stage};

/// 步骤展示元数据
const STAGES: [(Stage, &str); 4] = [
    (Stage::Enum, "硬件枚举"),
    (Stage::Driver, "驱动检查"),
    (Stage::Func, "功能自检"),
    (Stage::Sample, "稳定性采样"),
];

#[derive(Clone, Debug)]
pub struct GuiOptions {
    pub samples: u32,
    pub auto: bool,
    pub close_secs: u32,
}

/// 弹窗运行结果
pub struct GuiResult {
    pub status: bool,
    pub content: String,
}

/// 步骤当前状态
#[derive(Clone, Debug)]
enum StepState {
    Waiting,
    Running,
    Ok(String),
    Fail(String),
    Skipped,
}

/// 检测线程 -> GUI 线程的消息
enum Msg {
    Progress(Stage, Option<bool>, String),
    Done(bool, String),
}

/// GUI 应用状态
struct App {
    opts: GuiOptions,
    steps: Vec<(Stage, &'static str, StepState)>,
    detail: String,
    result: Option<bool>,
    countdown: Option<u32>,
    rx: Receiver<Msg>,
    last_tick: std::time::Instant,
    result_slot: std::sync::Arc<std::sync::Mutex<Option<GuiResult>>>,
}

impl App {
    fn new(opts: GuiOptions, rx: Receiver<Msg>, result_slot: std::sync::Arc<std::sync::Mutex<Option<GuiResult>>>) -> Self {
        let steps = STAGES.iter().map(|&(s, label)| (s, label, StepState::Waiting)).collect();
        Self {
            opts,
            steps,
            detail: String::new(),
            result: None,
            countdown: None,
            rx,
            last_tick: std::time::Instant::now(),
            result_slot,
        }
    }

    fn handle_progress(&mut self, stage: Stage, ok: Option<bool>, detail: String) {
        if let Some(slot) = self.steps.iter_mut().find(|(s, _, _)| *s == stage) {
            slot.2 = match ok {
                None => StepState::Running,
                Some(true) => StepState::Ok(detail),
                Some(false) => StepState::Fail(detail),
            };
        }
    }

    fn finish(&mut self, status: bool, content: String) {
        self.result = Some(status);
        self.detail = content;
        // 提前返回的分支（仅核显/无独显等）：未执行到的步骤标记为跳过，避免误以为"没测完"
        for (_, _, state) in &mut self.steps {
            if matches!(state, StepState::Waiting) {
                *state = StepState::Skipped;
            }
        }
        if self.opts.auto {
            // 倒计时起点：从检测完成这一刻开始算，重置 last_tick
            self.countdown = Some(self.opts.close_secs);
            self.last_tick = std::time::Instant::now();
        }
    }

    fn tick(&mut self, now: std::time::Instant) {
        // 处理检测线程消息
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Progress(stage, ok, detail) => self.handle_progress(stage, ok, detail),
                Msg::Done(status, content) => self.finish(status, content),
            }
        }
        // 倒计时：按实际流逝秒数扣减（慢渲染机器上若按帧递减会被放大数倍）
        if let Some(n) = self.countdown.as_mut() {
            let secs = now.duration_since(self.last_tick).as_secs() as u32;
            if secs > 0 {
                self.last_tick = now;
                *n = n.saturating_sub(secs);
            }
        }
    }
}

impl eframe::App for App {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // 关闭前把结果写入共享槽
        if let Some(mut slot) = self.result_slot.lock().ok() {
            *slot = Some(GuiResult { status: self.result.unwrap_or(false), content: self.detail.clone() });
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.tick(std::time::Instant::now());
        if self.countdown == Some(0) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        // 检测中/倒计时期间持续请求重绘（慢渲染、无输入事件时也保证推进）
        ctx.request_repaint_after(Duration::from_millis(200));

        // 内容区可滚动：小分辨率/小窗口下不裁切
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("显卡功能性测试").size(20.0).strong());
        });
        ui.add_space(8.0);

        // ---- 步骤区 ----
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (_, label, state) in &self.steps {
                ui.horizontal(|ui| {
                    let (icon, color) = match state {
                        StepState::Waiting => ("⏸", Color32::GRAY),
                        StepState::Running => ("⏳", Color32::YELLOW),
                        StepState::Ok(_) => ("✅", Color32::from_rgb(0x2E, 0x9B, 0xE8)),
                        StepState::Fail(_) => ("❌", Color32::from_rgb(0xD9, 0x30, 0x40)),
                        StepState::Skipped => ("⏭", Color32::GRAY),
                    };
                    ui.label(RichText::new(icon).size(15.0).color(color));
                    ui.label(RichText::new(*label).strong());
                    let detail = match state {
                        StepState::Waiting => "等待中...".to_string(),
                        StepState::Running => "检测中...".to_string(),
                        StepState::Ok(d) | StepState::Fail(d) => d.clone(),
                        StepState::Skipped => "跳过（不适用）".to_string(),
                    };
                    ui.label(RichText::new(detail).color(color));
                });
            }
        });
        ui.add_space(8.0);

        // ---- 明细区 ----
        if !self.detail.is_empty() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let mut text = self.detail.clone();
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(8)
                        .interactive(false),
                );
            });
            ui.add_space(8.0);
        }

        // ---- 结果区 ----
        ui.vertical_centered(|ui| {
            let (text, color) = match self.result {
                None => ("检测中...".to_string(), Color32::GRAY),
                Some(true) => ("PASS".to_string(), Color32::from_rgb(0x34, 0xC7, 0x59)), // 绿色
                Some(false) => ("FAIL".to_string(), Color32::from_rgb(0xD9, 0x30, 0x40)),
            };
            ui.label(RichText::new(text).size(30.0).strong().color(color));
        });
        ui.add_space(8.0);

        // ---- 交互区 ----
        ui.vertical_centered(|ui| {
            match (&self.result, self.opts.auto) {
                (Some(_), true) => {
                    let n = self.countdown.unwrap_or(0);
                    ui.label(RichText::new(format!("检测完成，{n} 秒后自动关闭...")).color(Color32::GRAY));
                }
                (Some(_), false) => {
                    if ui.add_sized([140.0, 34.0], egui::Button::new(RichText::new("确认").size(16.0).strong())).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                (None, _) => {
                    ui.label(RichText::new("正在检测，请稍候...").color(Color32::GRAY));
                }
            }
        });
        ui.add_space(8.0);
        });
    }
}

/// 在后台线程执行检测（不阻塞 GUI）
fn spawn_detect(opts: &GuiOptions, tx: Sender<Msg>) {
    let samples = opts.samples;
    let tx_done = tx.clone();
    std::thread::spawn(move || {
        let cb: detect::ProgressCb = Box::new(move |stage, ok, detail| {
            let _ = tx.send(Msg::Progress(stage, ok, detail));
        });
        let (status, content) = detect::detect(samples, Some(cb));
        let _ = tx_done.send(Msg::Done(status, content));
    });
}

/// 加载 CJK 字体（Windows 微软雅黑 / Linux Noto CJK / 文泉驿）
fn load_cjk_font(ctx: &egui::Context) {
    let candidates = [
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simhei.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ];
    for path in candidates {
        if let Ok(data) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert("cjk".into(), egui::FontData::from_owned(data).into());
            fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0, "cjk".into());
            ctx.set_fonts(fonts);
            return;
        }
    }
}

/// 启动 GUI 弹窗检测，返回 (status, content)。
/// 检测在后台线程执行；GUI 完成后窗口关闭，主线程返回。
pub fn run_gui(opts: GuiOptions) -> GuiResult {
    let (tx, rx) = mpsc::channel();
    spawn_detect(&opts, tx.clone());

    // 结果通过共享槽回传（app 被 move 进 run_native 闭包，无法在外部读取）
    let result_slot: std::sync::Arc<std::sync::Mutex<Option<GuiResult>>> = Default::default();
    let slot = result_slot.clone();
    let app = App::new(opts.clone(), rx, slot);
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720.0, 600.0])
            .with_min_inner_size([560.0, 480.0])
            .with_title("显卡功能性测试")
            .with_resizable(true),
        ..Default::default()
    };

    let _ = eframe::run_native(
        "gpu-test",
        native_options,
        Box::new(move |cc| {
            load_cjk_font(&cc.egui_ctx);
            cc.egui_ctx.set_theme(egui::Theme::Dark);
            Ok(Box::new(app))
        }),
    );

    // 窗口关闭后读取结果
    result_slot.lock().unwrap().take().unwrap_or(GuiResult { status: false, content: "GUI 未完成即关闭".into() })
}
