//! Templates of the findings of an installed game (SPEC-05 §4.7 step 2):
//! the context tokens of a translated manifest path are replaced, so that the
//! same entry of two games gives two `FindingId`s and the finding resolves
//! without the game context.

use sk_core::template::{PathTemplate, ResolveContext, Token};

/// The finding template of a translated entry of an installed game:
/// `{STORE_GAME_ID}` and `{GAME_DIR_NAME}` become their values as text
/// ([`with_values`]), then a leading `{GAME_DIR}` becomes the template of the
/// install folder ([`in_install_dir`]).
///
/// `None` when a value cannot be written as text of the template.
pub(super) fn finding_template(
    template: PathTemplate,
    ctx: &ResolveContext,
    install_dir: &PathTemplate,
) -> Option<PathTemplate> {
    let template = with_values(template, ctx)?;
    Some(in_install_dir(template, install_dir))
}

/// `template` with every `{STORE_GAME_ID}` and `{GAME_DIR_NAME}` replaced by
/// the value of `ctx` as text (`<storeGameId>` / `<game>` of two games must
/// not give one `FindingId`).
///
/// `None` (the entry is skipped, as a token-like name in `translate`) when a
/// needed value is missing, is empty, contains a separator, makes a `.` or
/// `..` segment, or reads as a token (`{APPDATA}`): such a value cannot stay
/// plain text, since templates have no escape syntax.
fn with_values(template: PathTemplate, ctx: &ResolveContext) -> Option<PathTemplate> {
    let tokens = template.tokens().count();
    let mut text = template.as_str().to_owned();
    let mut replaced = 0;
    for (token, value) in [
        (Token::StoreGameId, &ctx.store_game_id),
        (Token::GameDirName, &ctx.game_dir_name),
    ] {
        let count = template.tokens().filter(|t| *t == token).count();
        if count == 0 {
            continue;
        }
        let value = value.as_deref().filter(|v| is_plain_name(v))?;
        text = text.replace(&token.to_string(), value);
        replaced += count;
    }
    if replaced == 0 {
        return Some(template);
    }
    let substituted = PathTemplate::parse(&text).ok();
    let substituted = substituted.filter(|t| t.tokens().count() + replaced == tokens);
    if substituted.is_none() {
        tracing::debug!(
            template = template.as_str(),
            "store game id or game folder name is not plain text; entry skipped"
        );
    }
    substituted
}

/// A non-empty value without path separators.
fn is_plain_name(value: &str) -> bool {
    !value.is_empty() && !value.contains(['\\', '/'])
}

/// `template` with a leading `{GAME_DIR}` replaced by the template of the
/// install folder (`{STEAM}\steamapps\common\Celeste`): `<base>/Saves` of
/// two games must not give one `FindingId`, and the finding must resolve
/// without the game context.
fn in_install_dir(template: PathTemplate, install_dir: &PathTemplate) -> PathTemplate {
    let Some(rest) = template.as_str().strip_prefix("{GAME_DIR}") else {
        return template;
    };
    if !(rest.is_empty() || rest.starts_with('\\')) {
        return template;
    }
    PathTemplate::parse(&format!("{}{rest}", install_dir.as_str())).unwrap_or(template)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tpl(s: &str) -> PathTemplate {
        PathTemplate::parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    fn ctx(id: Option<&str>, name: Option<&str>) -> ResolveContext {
        ResolveContext {
            store_game_id: id.map(str::to_owned),
            game_dir_name: name.map(str::to_owned),
            ..ResolveContext::default()
        }
    }

    fn run(template: &str, ctx: &ResolveContext) -> Option<String> {
        let install = tpl(r"{STEAM}\steamapps\common\Celeste");
        finding_template(tpl(template), ctx, &install).map(|t| t.as_str().to_owned())
    }

    #[test]
    fn values_become_text() {
        let ctx = ctx(Some("504230"), Some("Celeste"));
        assert_eq!(
            run(r"{DOCUMENTS}\{GAME_DIR_NAME}\{STORE_GAME_ID}_save", &ctx).as_deref(),
            Some(r"{DOCUMENTS}\Celeste\504230_save")
        );
        assert_eq!(
            run(r"{GAME_DIR}\{GAME_DIR_NAME}\{STEAM_USERID}", &ctx).as_deref(),
            Some(r"{STEAM}\steamapps\common\Celeste\Celeste\{STEAM_USERID}")
        );
        // GUID-like braces stay text.
        let guid = ctx_name("{1ac14e77-02e7}");
        assert_eq!(
            run(r"{APPDATA}\{GAME_DIR_NAME}", &guid).as_deref(),
            Some(r"{APPDATA}\{1ac14e77-02e7}")
        );
    }

    fn ctx_name(name: &str) -> ResolveContext {
        ctx(None, Some(name))
    }

    #[test]
    fn templates_without_values_are_kept() {
        let ctx = ctx(None, None);
        assert_eq!(
            run(r"{APPDATA}\Game\{STEAM_USERID}", &ctx).as_deref(),
            Some(r"{APPDATA}\Game\{STEAM_USERID}")
        );
        assert_eq!(
            run(r"{GAME_DIR}\Saves", &ctx).as_deref(),
            Some(r"{STEAM}\steamapps\common\Celeste\Saves")
        );
    }

    #[test]
    fn values_that_are_not_plain_text_skip_the_entry() {
        for name in ["{APPDATA}", "{TEMP}", "a\\b", "a/b", "..", ".", ""] {
            assert_eq!(
                run(r"{DOCUMENTS}\{GAME_DIR_NAME}\s", &ctx_name(name)),
                None,
                "{name:?}"
            );
        }
        assert_eq!(run(r"{DOCUMENTS}\{STORE_GAME_ID}", &ctx(None, None)), None);
    }
}
