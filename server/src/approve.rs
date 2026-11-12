//! The minimal pairing approval page.
//!
//! Rule 10 says if a feature needs a browser it does not belong on the device.
//! Pairing is therefore approved on the other side, from a phone or laptop, on
//! this one small server-rendered page. No JavaScript, no assets, no cookies:
//! HTTP Basic auth plus a per-process CSRF token.

use crate::store::PairingRecord;

/// Render the approval page. `message` is shown above the list after an
/// approval attempt.
pub fn render_page(
    label: &str,
    fingerprint: &str,
    pending: &[PairingRecord],
    csrf: &str,
    message: Option<&str>,
    now: i64,
) -> String {
    let mut rows = String::new();
    for pairing in pending {
        let remaining = (pairing.expires_at - now).max(0);
        let minutes = (remaining + 59) / 60;
        rows.push_str(&format!(
            "<tr><td class=\"code\">{}</td><td>{}</td><td>{} min</td>\
             <td><form method=\"post\" action=\"/approve\">\
             <input type=\"hidden\" name=\"csrf\" value=\"{}\">\
             <input type=\"hidden\" name=\"pair_id\" value=\"{}\">\
             <button type=\"submit\">Approve</button></form></td></tr>\n",
            escape(&pairing.code),
            escape(&pairing.device_name),
            minutes,
            escape(csrf),
            pairing.pair_id
        ));
    }
    if rows.is_empty() {
        rows.push_str(
            "<tr><td colspan=\"4\" class=\"muted\">No pending pairing codes.</td></tr>\n",
        );
    }

    let message_html = message
        .map(|message| format!("<p class=\"message\">{}</p>\n", escape(message)))
        .unwrap_or_default();
    let short_fingerprint = if fingerprint.len() > 16 {
        format!("{}…", &fingerprint[..16])
    } else {
        fingerprint.to_string()
    };

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Carrel server — pairing</title>
<style>
  :root {{ color-scheme: light dark; }}
  body {{ font-family: system-ui, sans-serif; margin: 0; padding: 2rem 1rem;
          background: #f6f4ef; color: #1c1a17; }}
  main {{ max-width: 42rem; margin: 0 auto; }}
  h1 {{ font-size: 1.6rem; margin: 0 0 .25rem; }}
  h2 {{ font-size: 1.1rem; margin: 2rem 0 .75rem; }}
  .muted {{ color: #6b655c; font-size: .85rem; }}
  .message {{ padding: .75rem 1rem; background: #e7efe3; border-radius: .5rem; }}
  table {{ width: 100%; border-collapse: collapse; background: #fff;
           border-radius: .5rem; overflow: hidden; }}
  th, td {{ text-align: left; padding: .6rem .7rem; border-bottom: 1px solid #e6e1d8; }}
  th {{ font-size: .75rem; text-transform: uppercase; letter-spacing: .05em; color: #6b655c; }}
  .code {{ font-family: ui-monospace, monospace; font-size: 1.1rem; letter-spacing: .08em; }}
  button {{ font: inherit; padding: .45rem .9rem; border-radius: .4rem;
            border: 1px solid #1c1a17; background: #1c1a17; color: #fff; }}
  form.code-form {{ display: flex; gap: .5rem; }}
  input[type=text] {{ font: inherit; padding: .45rem .6rem; width: 9rem;
                      border: 1px solid #b6afa4; border-radius: .4rem;
                      text-transform: uppercase; letter-spacing: .1em; }}
  @media (prefers-color-scheme: dark) {{
    body {{ background: #171614; color: #ece8e0; }}
    table {{ background: #201e1b; }}
    th, td {{ border-color: #34312c; }}
    .muted {{ color: #9a9388; }}
    .message {{ background: #24311f; }}
    button {{ background: #ece8e0; color: #171614; border-color: #ece8e0; }}
    input[type=text] {{ background: #201e1b; color: #ece8e0; border-color: #4b463e; }}
  }}
</style>
</head>
<body>
<main>
  <h1>Carrel server</h1>
  <p class="muted">{label} · certificate {fingerprint}</p>
  {message}
  <h2>Waiting for approval</h2>
  <table>
    <thead><tr><th>Code</th><th>Device</th><th>Expires</th><th></th></tr></thead>
    <tbody>
{rows}    </tbody>
  </table>
  <h2>Approve by code</h2>
  <p class="muted">Type the code shown on the device.</p>
  <form class="code-form" method="post" action="/approve">
    <input type="hidden" name="csrf" value="{csrf}">
    <input type="text" name="code" inputmode="text" autocomplete="off"
           autocapitalize="characters" spellcheck="false" placeholder="K7Q-4MZ" required>
    <button type="submit">Approve</button>
  </form>
</main>
</body>
</html>
"#,
        label = escape(label),
        fingerprint = escape(&short_fingerprint),
        message = message_html,
        rows = rows,
        csrf = escape(csrf),
    )
}

/// Escape text for HTML element and attribute contexts.
pub fn escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(character),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrel_protocol::{new_pair_id, PairStatus};

    #[test]
    fn escapes_html() {
        assert_eq!(
            escape("<script>\"x\" & 'y'</script>"),
            "&lt;script&gt;&quot;x&quot; &amp; &#39;y&#39;&lt;/script&gt;"
        );
    }

    #[test]
    fn page_lists_pending_codes() {
        let pending = vec![PairingRecord {
            pair_id: new_pair_id(),
            device_name: "<X250>".into(),
            device_pubkey: "ab".into(),
            code: "K7Q-4MZ".into(),
            status: PairStatus::Pending,
            token: None,
            created_at: 100,
            expires_at: 700,
        }];
        let html = render_page(
            "Study",
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            &pending,
            "csrf-token",
            Some("Approved <X250>"),
            400,
        );
        assert!(html.contains("K7Q-4MZ"));
        assert!(html.contains("&lt;X250&gt;"));
        assert!(!html.contains("<X250>"));
        assert!(html.contains("Approved &lt;X250&gt;"));
        assert!(html.contains("name=\"csrf\" value=\"csrf-token\""));
        assert!(html.contains("Approve"));
    }

    #[test]
    fn empty_page_says_so() {
        let html = render_page("Study", "abc", &[], "t", None, 0);
        assert!(html.contains("No pending pairing codes."));
    }
}
