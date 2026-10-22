//! VOS locale snapshot plus live broadcast, including bridges injected after app startup.
use super::bridge;
use photocraft_ui_egui::{PhotocraftApp, i18n, served_fonts};
use std::cell::RefCell;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
#[derive(Default)]
struct State {
    connected: bool,
    started: Option<f64>,
    revision: u64,
    current: Option<String>,
    listener: Option<Closure<dyn FnMut(JsValue)>>,
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
fn accept(value: &JsValue) {
    if let Some(locale) = bridge::get(value, "locale").ok().and_then(|v| v.as_string()).and_then(|v| super::model::locale_code(&v).map(str::to_owned)) {
        STATE.with(|s| s.borrow_mut().current = Some(locale));
    }
}
pub fn sync(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if !STATE.with(|s| s.borrow().connected)
        && let Some(object) = bridge::bridge()
        && let Ok(broadcast) = bridge::get(&object, "broadcast")
        && let Ok(on) = bridge::get(&broadcast, "on").and_then(|v| v.dyn_into::<js_sys::Function>().map_err(|_| "invalid locale event".into()))
    {
        let repaint = ctx.clone();
        let listener = Closure::<dyn FnMut(JsValue)>::new(move |value| {
            STATE.with(|s| s.borrow_mut().revision += 1);
            accept(&value);
            repaint.request_repaint();
        });
        if on.call2(&broadcast, &JsValue::from_str("locale-change"), listener.as_ref()).is_ok() {
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.connected = true;
                s.listener = Some(listener);
            });
            let revision = STATE.with(|s| s.borrow().revision);
            let repaint = ctx.clone();
            spawn_local(async move {
                if let Ok(promise) = bridge::call(&object, "getState", &JsValue::UNDEFINED)
                    && let Ok(value) = JsFuture::from(promise).await
                    && STATE.with(|s| s.borrow().revision == revision)
                {
                    accept(&value);
                    repaint.request_repaint();
                }
            });
        }
    }
    if let Some(locale) = STATE.with(|s| s.borrow().current.clone())
        && app.session.prefs().interface.language != locale
        && app.run("prefs.set", serde_json::json!({"path":"interface.language","value":locale})).is_ok()
    {
        served_fonts::request_for_languages([locale.as_str()]);
        i18n::sync_context(ctx, &locale);
        ctx.request_repaint();
    }
    let now = ctx.input(|i| i.time);
    let waiting = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let started = *s.started.get_or_insert(now);
        !s.connected && now - started < 10.0
    });
    if waiting {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}
