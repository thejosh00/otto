//! The two things a page watches rather than asks for: a journal being written (`otto logs -f`)
//! and a wake running (`otto attach`). Both are server-sent events, polled server-side at the
//! same pace the CLI polls — a run writes a few lines a minute, so nothing cleverer is needed.

use super::handlers::LogParams;
use super::{ApiError, AppState};
use crate::core::LiveView;
use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::stream::{self, Stream, StreamExt};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

type Events = std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

fn json_event(name: &str, value: &impl serde::Serialize) -> Event {
    Event::default().event(name).data(serde_json::to_string(value).unwrap_or_default())
}

/// `header` once, then `lines` (a JSON array) as the journal grows, starting with the tail the
/// query asks for.
pub async fn logs_stream(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<LogParams>,
) -> Response {
    let offset = state.offset;
    let query = params.query();
    let page = match tokio::task::spawn_blocking(move || crate::core::read_logs(&id, &query, offset)).await {
        Ok(Ok(page)) => page,
        Ok(Err(err)) => return ApiError(err).into_response(),
        Err(join) => return ApiError(crate::error::OttoError::new(join.to_string(), 4)).into_response(),
    };
    let first = vec![Ok(json_event("header", &page.header)), Ok(json_event("lines", &page.lines))];
    let follow = stream::unfold(page.cursor, |mut cursor| async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let (back, fresh) = tokio::task::spawn_blocking(move || {
                let fresh = cursor.poll();
                (cursor, fresh)
            })
            .await
            .ok()?;
            cursor = back;
            if !fresh.is_empty() {
                return Some((Ok(json_event("lines", &fresh)), cursor));
            }
        }
    });
    let events: Events = Box::pin(stream::iter(first).chain(follow));
    Sse::new(events).keep_alive(KeepAlive::default()).into_response()
}

/// What the running wake looks like, re-sent whenever it changes: `pane` (a tmux pane, colour
/// escapes kept) or `log` (the tail of `wake.log`), then `ended` once no wake is running.
pub async fn live_stream(Path(id): Path<String>) -> Response {
    let resolved = match tokio::task::spawn_blocking(move || crate::paths::resolve_run_id(&id)).await {
        Ok(Ok(id)) => id,
        Ok(Err(err)) => return ApiError(err).into_response(),
        Err(join) => return ApiError(crate::error::OttoError::new(join.to_string(), 4)).into_response(),
    };
    let events = stream::unfold(Some((resolved, None::<LiveView>)), |state| async move {
        let (id, mut last) = state?;
        loop {
            let probe = id.clone();
            let view = tokio::task::spawn_blocking(move || crate::core::live_view(&probe, &mut crate::exec::RealExec))
                .await
                .ok()?;
            if last.as_ref() != Some(&view) {
                let event = match &view {
                    LiveView::Pane(text) => json_event("pane", text),
                    LiveView::Log(text) => json_event("log", text),
                    LiveView::Ended(message) => {
                        // Last event: the stream ends here, and the page stops asking.
                        return Some((Ok::<_, Infallible>(json_event("ended", message)), None));
                    }
                };
                last = Some(view);
                return Some((Ok(event), Some((id, last))));
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    let events: Events = Box::pin(events);
    Sse::new(events).keep_alive(KeepAlive::default()).into_response()
}
