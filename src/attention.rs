//! User decisions that are currently waiting in conversations.

use std::collections::BTreeMap;

use crate::engine::View;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Decision {
    Tool(u64),
    Browser(u64),
    Website,
    Location(u64),
}

impl Decision {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Tool(_) => "Tool approval",
            Self::Browser(_) | Self::Website => "Browser access",
            Self::Location(_) => "Location choice",
        }
    }
}

pub(crate) fn waiting(view: &View) -> BTreeMap<u64, Vec<Decision>> {
    let state = &view.state;
    let mut waiting = BTreeMap::<u64, Vec<Decision>>::new();
    let mut add = |chat, decision| {
        if state.thread_data(chat).is_some() {
            waiting.entry(chat).or_default().push(decision);
        }
    };
    for request in &state.requests {
        if !request.approved {
            add(request.chat, Decision::Tool(request.id));
        }
    }
    if let Some((id, stage)) = view.web_region_prompt
        && stage != crate::location::WebRegionPrompt::Locate
    {
        add(state.chat_id, Decision::Location(id));
    }
    if crate::engine::can_request_browser(state) {
        let disabled = &state.preferences.disabled_browsers;
        let policy = crate::browser_link::Policy::from_preferences(&state.preferences);
        let selected = view.browser_tab_origin.as_deref();
        let readers = view.browser_clients.iter().filter(|client| !disabled.contains(&client.browser));
        for (&chat, read) in &state.browser_reads {
            let destination = read.destination.as_ref();
            let candidates = readers.clone().filter(|client| client.can_connect(destination)).count();
            let awaiting_site = destination.is_some_and(|page| {
                read.approved_origin.as_deref() != Some(page.origin().ascii_serialization().as_str())
                    && policy.rule_for(page).is_none()
            });
            let selecting_tab = destination.is_none() && candidates != 1;
            let setup_needed = candidates == 0 && !readers.clone().any(|client| {
                client.ready && client.regular_profile() && client.up_to_date()
            });
            let browser_permission = view.browser_pairing == Some((chat, false)) && selected.is_some();
            if awaiting_site || selecting_tab || setup_needed || browser_permission {
                add(chat, Decision::Browser(read.id));
            }
        }
        if let Some((chat, _)) = view.browser_pairing
            && !state.browser_reads.contains_key(&chat)
            && selected.is_some() && selected == view.browser_site_request.as_deref()
        {
            add(chat, Decision::Website);
        }
    }
    waiting
}
