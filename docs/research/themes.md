
# Sphinx HTML Theme Support Research for sphinx-ultra (mid-2026)

## 0. Popularity verification (data collected 2026-08-06)

PyPI downloads via pypistats.org; stars/licenses/activity via GitHub API.

| Theme | PyPI downloads / month | GitHub stars | Repo | License | Last push |
|---|---:|---:|---|---|---|
| alabaster | 29,818,650 | 777 | sphinx-doc/alabaster | BSD-3-Clause | 2024-07 (stable/dormant) |
| sphinx-rtd-theme | 14,482,056 | 5,076 | readthedocs/sphinx_rtd_theme | MIT | 2026-01 |
| pydata-sphinx-theme | 4,691,219 (v0.20.0) | 772 | pydata/pydata-sphinx-theme | BSD-3-Clause | 2026-08 |
| sphinx-book-theme | 1,222,725 (v1.4.0) | 493 | executablebooks/sphinx-book-theme | BSD-3-Clause | 2026-08 |
| furo | ~1,051,113 | 3,554 | pradyunsg/furo | MIT | 2026-08 |
| sphinx-immaterial | 237,564 (v0.13.9) | 259 | jbms/sphinx-immaterial | MIT (custom notice) | 2026-08 |
| shibuya | 199,578 | 308 | lepture/shibuya | BSD-3-Clause | 2026-07 |
| sphinx-press-theme | (negligible) | 120 | schettino72/sphinx_press_theme | custom | 2024-04 (unmaintained) |

Caveat: alabaster's download count is inflated — it is a hard dependency of Sphinx itself, so every `pip install sphinx` pulls it. Its *usage* share is smaller, but it is the built-in default theme, so support is mandatory regardless.

### Selected top 5 (in recommended implementation order)

1. **alabaster** — Sphinx's default; any project without `html_theme` set uses it. Simplest theme; ideal for bootstrapping the theme layer.
2. **sphinx-rtd-theme** — highest real-world usage (14.5M/mo, 5.1k stars); the "docs look" for a decade of projects; nearly pure-Jinja (easiest big win).
3. **furo** — the default choice for new Python projects (pip, attrs, urllib3…); 3.5k stars; huge mindshare relative to downloads.
4. **pydata-sphinx-theme** — powers NumPy, Pandas, SciPy, Matplotlib, Jupyter docs; 4.7M/mo; also a prerequisite for #5.
5. **sphinx-book-theme** — Jupyter Book ecosystem; inherits pydata-sphinx-theme, so ~70% of the work is shared with #4.

Excluded: **shibuya** (strong 6th candidate: modern, pure-Jinja, BSD, but 200k/mo — revisit after the top 5); **sphinx-immaterial** (deep Python coupling: its own object-description/apigen machinery, custom admonition/tab directives, mkdocs-material build pipeline — worst effort/benefit ratio); **press/awesome** (unmaintained or niche).

---

## 1. alabaster

### 1.1 Packaging
- Ships as a plain Python package; registered via the `sphinx.html_themes` entry point (`alabaster = alabaster`). Also historically loadable via `html_theme_path`.
- **`theme.conf`** (INI) — not theme.toml. `inherit = basic`, `stylesheet = basic.css, alabaster.css`, `sidebars = about.html, searchfield.html, navigation.html, relations.html, donate.html`, `pygments_style = alabaster.support.Alabaster` (a **Python Pygments style class** — see 1.5).
- Inheritance chain: `alabaster → basic`.
- Templates it adds/overrides: `layout.html` (minor), sidebar partials `about.html`, `donate.html`, `navigation.html`, `relations.html`, `searchfield.html`.
- Static assets: **`alabaster.css_t`** (a *Jinja-templated* stylesheet rendered at build time with `theme_*` variables), `custom.css` (empty user hook), `github-banner.svg`. No bundled JS.

### 1.2 Templating requirements
- Standard `basic` theme contract: `pathto()`, `hasdoc()`, `toctree()`, `sidebars` loop with dynamic `{% include %}`, `css_tag`/`js_tag`, `_()`/`{% trans %}` i18n, `relbar`, `sourcename`, `metatags`, `body`.
- The key extra requirement is the **`.css_t` pipeline**: Sphinx renders `_t`-suffixed static files as Jinja templates against the theme-options context (`theme_page_width`, `theme_sidebar_width`, ~30 color/font variables). sphinx-ultra must implement this generic mechanism.
- Python-side context injection is trivial (`alabaster_version`, defaulted html_context keys).

### 1.3 Theme options (full surface, from theme.conf)
`description`, `logo`, `logo_name`, `logo_text_align`, `touch_icon`, `page_width`, `sidebar_width`, `body_min_width`, `fixed_sidebar`, `github_user`, `github_repo`, `github_button`, `github_type`, `github_count`, `github_banner`, `badge_branch`, `codecov_button`, `travis_button`, `donate_url`, `opencollective`, `opencollective_button_color`, `gittip_user`/`gratipay_user`, `tidelift_url`, `analytics_id`, `canonical_url`, `extra_nav_links`, `show_powered_by`, `show_related`, `show_relbar_top/bottom`, `show_relbars`, `relbar_border`, `sidebar_collapse`, `sidebar_includehidden`, plus ~40 color/typography variables (`gray_1..3`, `pink_1..3`, `base_bg`, `body_text`, `link`, `footer_text`, font settings, …) consumed by `alabaster.css_t`.

### 1.4 JS behaviors
- None bundled. Optional GitHub star/watch buttons load a third-party script when `github_button` is on. Relies entirely on Sphinx core JS: `documentation_options`, `doctools.js`, `sphinx_highlight.js`, `searchtools.js` + `searchindex.js`.

### 1.5 Rust renderability
- Templates are simple Jinja: `extends`/`block`/`include`, `{{ _() }}`, a few `{% trans %}` blocks inherited from `basic`. **minijinja handles everything except `{% trans %}`** (Jinja2 i18n extension tags) — solve once with a preprocessing pass that rewrites `{% trans x=y %}…{% endtrans %}` into `gettext()` calls (see cross-cutting section).
- `pygments_style = alabaster.support.Alabaster` is Python: sphinx-ultra must either vendor that style's color table into its highlighter or emit an equivalent pre-generated `pygments.css`.
- Licensing: BSD-3-Clause — vendoring templates + assets with the license text is fine. The inherited `basic` theme is part of Sphinx (BSD-2-Clause) — also vendorable with notice.

### 1.6 Strategy
**Vendor + render with a Rust Jinja engine (minijinja).** Alabaster is the cheapest full-fidelity target and forces sphinx-ultra to build the generic machinery every other theme needs (theme.conf parsing, inheritance resolution, sidebars, `_t` static templating, options → `theme_*` context).

---

## 2. sphinx-rtd-theme

### 2.1 Packaging
- Python package, entry point `sphinx.html_themes`; also registers itself as a Sphinx *extension* (activates `sphinxcontrib-jquery` and locale catalogs).
- **`theme.conf`**: `inherit = basic`, `stylesheet = css/theme.css`, `pygments_style = default`.
- Chain: `sphinx_rtd_theme → basic`.
- Templates: `layout.html` (major rewrite of basic's), `breadcrumbs.html`, `footer.html`, `versions.html` (RTD flyout), `searchbox.html`, `search.html`; ships compiled `locale/` catalogs (its templates are translated).
- Static assets (in the wheel, built from SASS/webpack): `css/theme.css`, `css/badge_only.css`, `js/theme.js`, `js/versions.js`, bundled webfonts: **Lato** (OFL), **Roboto Slab** (Apache-2.0), **FontAwesome 4** (OFL 1.1 fonts + MIT CSS).

### 2.2 Templating requirements
- Heavy use of the standard context: multiple `toctree()` calls with kwargs — `toctree(maxdepth=theme_navigation_depth|toint, collapse=theme_collapse_navigation|tobool, includehidden=theme_includehidden|tobool, titles_only=theme_titles_only|tobool)` — plus `pathto`, `hasdoc`, `next`/`prev`, `parents`, `title`, `master_doc`/`root_doc`, `display_toc`/`toc`, `page_source_suffix`, `sourcename`, `last_updated`, `show_sphinx`, `show_copyright`.
- **html_context contract** (largest of any theme): `display_github`/`github_user`/`github_repo`/`github_version`/`conf_py_path`/`source_suffix` (Edit-on-GitHub links), same for `display_gitlab`/`display_bitbucket`, `READTHEDOCS`, `current_version`, `versions`, `downloads`, `theme_display_version` (removed in 3.0), plus RTD-injected flyout data.
- Sphinx filters `toint`/`tobool`, i18n `{% trans %}` blocks throughout `breadcrumbs.html`/`footer.html`/`versions.html`.
- No meaningful Python-side page-context hooks — the theme is ~pure Jinja. This makes it the easiest *popular* theme to render faithfully.

### 2.3 Theme options (full surface, from theme.conf 3.x)
`canonical_url` (deprecated), `analytics_id`, `analytics_anonymize_ip`, `collapse_navigation` (default True), `sticky_navigation` (True), `navigation_depth` (4), `includehidden` (True), `titles_only`, `logo_only`, `prev_next_buttons_location` (bottom), `style_external_links`, `style_nav_header_background`, `vcs_pageview_mode` (blob/edit/raw), `flyout_display` (hidden), `version_selector` (True), `language_selector` (True). (`display_version` was removed in 3.0.)

### 2.4 JS behaviors
- `js/theme.js` (jQuery, via sphinxcontrib-jquery): mobile nav hamburger toggle (`wy-nav-*` classes), sticky sidebar with scroll-position preservation, dynamic expand/collapse buttons injected into the sidebar toctree, table wrapping (`wy-table-responsive`).
- `js/versions.js` + `versions.html`: Read the Docs flyout menu / version + language selector (reads RTD-injected context or the `readthedocs-addons` data API in 3.x).
- No theme toggle (dark mode via RTD addons only), no copy button (users add sphinx-copybutton), search is stock Sphinx `searchtools.js`.

### 2.5 Rust renderability
- Jinja features: extends/blocks/include, macros from `basic`, filters `e`, `tobool`, `toint`, `striptags`, `title`, set-statements, `{% trans %}` — all fine in minijinja after the trans-preprocess; `toctree()`/`pathto()` provided as Rust callables.
- The hard part is not the templates but reproducing Sphinx's **global toctree resolution** (collapse/titles_only/maxdepth semantics and `current` classes) — sphinx-ultra needs this anyway for every theme.
- jQuery dependency is self-contained (vendor `jquery.js` from sphinxcontrib-jquery, MIT).
- Licensing: MIT theme + OFL/Apache fonts — vendoring into the binary (or an assets pack) is unproblematic with license files preserved.

### 2.6 Strategy
**Vendor + render templates with minijinja.** Highest-fidelity-per-effort of all five. Do not reimplement natively — pixel parity with `theme.css` and the exact `wy-*` DOM matters because thousands of projects override it with `custom.css` keyed to those class names.

---

## 3. furo

### 3.1 Packaging
- Python package; entry point `sphinx.html_themes` (`furo = furo`); also acts as an extension (registers itself, forces `pygments_dark_style`).
- **`theme.conf`**: `inherit = basic-ng`, `stylesheet = styles/furo.css`, `pygments_style = a11y-light`, `sidebars = sidebar/brand.html, sidebar/search.html, sidebar/scroll-start.html, sidebar/navigation.html, sidebar/ethical-ads.html, sidebar/scroll-end.html, sidebar/variant-selector.html`.
- Chain: **`furo → basic-ng → basic`** — note the extra hop: `sphinx-basic-ng` (MIT, same author) provides a modernized skeleton (`sections/`, `components/` partials, `base.html`). sphinx-ultra must vendor basic-ng too.
- Templates: `base.html`, `page.html` (main layout), `layout.html`, `search.html`, `genindex.html`, `domainindex.html`, `globaltoc.html`/`localtoc.html`, `partials/` (icons, toc, edit-this-page), `components/edit-this-page.html`, `sidebar/*` partials.
- Static assets are **built at release** (not in git tree): `styles/furo.css`, `styles/furo-extensions.css`, `scripts/furo.js` (+ source maps) — vendor from the wheel/sdist, not the repo. Icons are inline SVG (FontAwesome-derived) embedded in templates.

### 3.2 Templating requirements
- Uses basic-ng's section/component model: blocks like `site_meta`, `styles`, `scripts`, and furo-specific context values computed **in Python** via `html-page-context`:
  - `furo_navigation_tree` — furo takes Sphinx's `toctree(...)` HTML output and **rewrites it in Python** to inject `<input class="toctree-checkbox" type="checkbox">` + `<label>` pairs for its pure-CSS collapsible sidebar. sphinx-ultra must reimplement this HTML transform (straightforward with an HTML rewriter, or generate the structure natively from its own toctree model).
  - `furo_hide_toc` — computed from whether the local TOC has entries.
  - CSS-variable stylesheets generated from `light_css_variables`/`dark_css_variables` dict options (emitted as an inline `<style>`/`_static` asset).
  - Edit/view URL synthesis from `source_repository`/`source_branch`/`source_directory` (or explicit `source_edit_link`/`source_view_link` with `{filename}` interpolation).
- Otherwise standard: `pathto`, `_()`, theme_* options, `metatags`, `next`/`prev`.

### 3.3 Theme options (full surface, from theme.conf)
`announcement`, `light_css_variables` / `dark_css_variables` (dicts of CSS custom properties), `light_logo` / `dark_logo`, `sidebar_hide_name`, `footer_icons` (list of dicts: name/url/html/class), `top_of_page_button` (deprecated) / `top_of_page_buttons` (edit, view), `source_repository`, `source_branch`, `source_directory`, `source_edit_link`, `source_view_link`; plus `navigation_with_keys` inherited from basic. Also honors `pygments_style`/`pygments_dark_style` conf values.

### 3.4 JS behaviors
- `furo.js`: three-state **theme toggler** (auto → light → dark, persisted in `localStorage`, applied pre-paint via inline script to avoid FOUC), header shrink/"scrolled" state, back-to-top button visibility, **TOC scrollspy** (Gumshoe-based highlight of the right-sidebar TOC), sidebar scroll restoration.
- Sidebar collapse is CSS-only (the checkbox hack — no JS), which is why the Python toctree rewrite matters.
- Search is stock Sphinx. No bundled copy button or version switcher (RTD flyout via ethical-ads/variant-selector partials when hosted on RTD).

### 3.5 Rust renderability
- Templates use extends/include/blocks/macros and `{{ _() }}`; minijinja-compatible after the shared trans-preprocess. Dict-valued theme options (`*_css_variables`, `footer_icons`) mean the theme-options context must support **structured values, not just strings** (these come from `conf.py`, so sphinx-ultra's config layer must pass them through as JSON-like values).
- Python-side transforms (navigation tree rewrite, CSS variable emission, edit-URL synthesis) are small, well-scoped, and re-implementable in Rust (~300 lines total).
- Licensing: MIT (furo and sphinx-basic-ng). Vendoring the release-built CSS/JS is explicitly viable; keep FontAwesome attribution comment in CSS.

### 3.6 Strategy
**Vendor templates (furo + basic-ng) + minijinja, with a small native Rust shim** replicating furo's Python context hooks (nav-tree checkbox injection, CSS-variable emission, source links). Do not re-author the CSS/JS — ship the built assets verbatim.

---

## 4. pydata-sphinx-theme

### 4.1 Packaging
- Python package; entry point `sphinx.html_themes`; also an extension (registers event handlers, its own Jinja filters/functions, locale catalogs).
- **`theme.conf`**: `inherit = basic`, `stylesheet = styles/pydata-sphinx-theme.css` (note: CSS/JS are actually linked manually in `webpack-macros.html`, not only via the stylesheet key), `pygments_style = tango` (overridden at runtime by light/dark styles), `sidebars = sidebar-collapse, sidebar-nav-bs`.
- Chain: `pydata_sphinx_theme → basic`.
- Templates: `layout.html`, `search.html`, `sections/` (header, article, footer, sidebars), `components/` (~30 pluggable components: navbar-logo, navbar-nav, navbar-icon-links, theme-switcher, version-switcher, search-button/-field, page-toc, edit-this-page, sourcelink, breadcrumbs, copyright, sphinx-version, …), `webpack-macros.html` (hashed asset URLs).
- Static assets (webpack-built, present only in wheels): `styles/pydata-sphinx-theme.css`, `scripts/pydata-sphinx-theme.js`, `scripts/bootstrap.js` (Bootstrap 5 bundle), FontAwesome webfonts, vendored fonts.

### 4.2 Templating requirements
This theme has the **largest Python-side surface** of the five. Its templates call functions injected via `html-page-context`:
- `generate_header_nav_html(n_links_before_dropdown, dropdown_text)` — builds the top navbar from the *root* toctree, with overflow dropdown.
- `generate_toctree_html(kind, startdepth, show_nav_level, maxdepth, collapse, includehidden, titles_only)` — sidebar nav: takes Sphinx toctree HTML and rewrites it (BeautifulSoup) to add Bootstrap classes, `<details>`-style expand chevrons, and nav-level visibility classes.
- `generate_toc_html(kind)` — in-page TOC processed for `show_toc_level`.
- `get_edit_provider_and_url()` — GitHub/GitLab/Bitbucket edit URL from html_context (`github_user`, `github_repo`, `github_version`, `doc_path`).
- `unique_html_id`, `theme_get`/component-render helpers, icon-link normalization, version-switcher validation (`check_switcher` fetches the JSON at build time).
- html_context keys: `github_user`, `github_repo`, `github_version`, `gitlab_*`, `bitbucket_*`, `doc_path`, `default_mode` (light/dark/auto).
- Uses `{% trans %}`, `{{ _() }}`, dynamic component includes (each name in `navbar_start`, `secondary_sidebar_items`, etc. resolves to a template include — including **user-supplied templates** and per-page overrides via glob patterns).

### 4.3 Theme options (full surface, from theme.conf 0.20)
Layout slots: `navbar_start` (navbar-logo), `navbar_center` (navbar-nav), `navbar_end` (theme-switcher, navbar-icon-links), `navbar_persistent`, `article_header_start` (breadcrumbs), `article_header_end`, `article_footer_items`, `content_footer_items`, `primary_sidebar_end`, `secondary_sidebar_items` (page-toc, edit-this-page, sourcelink; supports per-page-glob dict), `footer_start` (copyright, sphinx-version), `footer_center`, `footer_end` (theme-version).
Navigation: `navigation_depth` (4), `show_nav_level` (1), `show_toc_level` (1), `collapse_navigation`, `navigation_with_keys`, `sidebar_includehidden`, `header_links_before_dropdown` (5), `header_dropdown_text` (More), `navbar_align` (content|left|right), `show_prev_next`.
Branding: `logo` (dict: `image_light`, `image_dark`, `text`, `link`, `alt_text`), `logo_link`, `announcement` (text or URL), `sticky_banners`, `show_version_warning_banner`.
Switcher: `switcher` (dict: `json_url`, `version_match`), `check_switcher`. Switcher JSON format: `[{"name", "version", "url", "preferred"}]`.
Links/icons: `icon_links` (list of dicts: name/url/icon/type/attributes), `icon_links_label`, `external_links` (list of dicts), `github_url`/`gitlab_url`/`bitbucket_url`/`twitter_url`.
Misc: `use_edit_page_button`, `analytics` (dict: google/plausible), `search_bar_text`, `search_as_you_type`, `disable_search`, `shorten_urls`, `back_to_top_button`, `surface_warnings`, `pygments_light_style` / `pygments_dark_style` (a11y styles).

### 4.4 JS behaviors
- Bootstrap 5 bundle: dropdowns, collapse, tooltips.
- `pydata-sphinx-theme.js`: three-state **theme switcher** (localStorage `mode`, `data-theme` attribute, pre-paint inline script), **version switcher** (client-side fetch of `switcher.json`, renders dropdown, injects "old/dev version" warning banner when `version_match` ≠ preferred), search shortcut (**Ctrl/Cmd+K** focuses/opens the search dialog), mobile off-canvas sidebars, in-page **TOC scrollspy**, dismissible + sticky announcement handling, back-to-top button, external-link decorations.
- Search UI is a styled overlay over stock Sphinx `searchtools.js` (plus optional `search_as_you_type`).

### 4.5 Rust renderability
- The Jinja itself (extends/blocks/macros/dynamic includes/trans) is minijinja-feasible, but templates are thin wrappers — **the real logic lives in the injected Python callables** (navbar/toctree/toc HTML generation via BeautifulSoup rewriting). Faithful support requires reimplementing those in Rust against sphinx-ultra's own toctree model (recommended: generate the Bootstrap-classed nav HTML natively rather than rewrite-after-render).
- Dict/list-valued options everywhere → structured theme-option context required.
- `check_switcher` (network fetch at build) should be a warn-only no-op or feature-gated in Rust.
- Licensing: BSD-3-Clause; bundles Bootstrap (MIT) and FontAwesome (OFL-1.1 fonts/CC-BY icons/MIT CSS) — all vendorable with a NOTICE file.

### 4.6 Strategy
**Hybrid: vendor templates + built assets, minijinja for rendering, native Rust reimplementation of the injected helper functions** (`generate_header_nav_html`, `generate_toctree_html`, `generate_toc_html`, edit-URL provider). This is the most work of the five but unlocks sphinx-book-theme nearly for free.

---

## 5. sphinx-book-theme

### 5.1 Packaging
- Python package; entry point `sphinx.html_themes`; extension registering its own `html-page-context` handlers and components.
- **`theme.conf`**: **`inherit = pydata_sphinx_theme`**, `stylesheet = styles/sphinx-book-theme.css`, `pygments_style = tango`, `sidebars = navbar-logo.html, icon-links.html, search-button-field.html, sbt-sidebar-nav.html`.
- Chain: `sphinx_book_theme → pydata_sphinx_theme → basic` (three-level inheritance — the theme loader must resolve transitively across *packages*).
- Templates: `layout.html`, `macros/`, `sections/`, `components/` — overrides PST components and adds: `article-header-buttons.html`, `toggle-primary-sidebar.html`, `sbt-sidebar-nav.html`, download/fullscreen/launch buttons, `author.html`, `last-updated.html`, `extra-footer.html`.
- Static: `styles/sphinx-book-theme.css`, `scripts/sphinx-book-theme.js` (webpack-built, wheel-only).

### 5.2 Templating requirements
- Everything PST requires (section 4.2), plus book-theme's own Python context: header-button preparation (`prep_header_buttons` — builds the source/edit/issues/download/fullscreen/launch button lists), **launch-button URL generation** (Binder/JupyterHub/Colab/Deepnote URLs from `launch_buttons` dict + `repository_url` + `path_to_docs` + notebook path), repository button URL synthesis, `translate` helpers for its locale catalogs.
- html_context: inherits PST's repo keys; `author`, `last_updated` used by footer components.

### 5.3 Theme options (full surface, from theme.conf 1.4)
All PST options, plus/overriding: `announcement`, `secondary_sidebar_items` (page-toc.html only), `toc_title` ("Contents"), `article_header_start` (toggle-primary-sidebar), `article_header_end` (article-header-buttons), `use_download_button` (True), `use_fullscreen_button` (True), `use_issues_button`, `use_source_button`, `use_repository_button`, `use_edit_page_button` (inherited), `path_to_docs`, `repository_url`, `repository_branch`, `repository_provider` (github|gitlab|bitbucket), `launch_buttons` (dict: `binderhub_url`, `jupyterhub_url`, `colab_url`, `deepnote_url`, `notebook_interface`, `thebe`), `home_page_in_toc`, `show_navbar_depth` (1), `max_navbar_depth` (4), `collapse_navbar`, `extra_footer`, `footer_content_items` (author, copyright, last-updated, extra-footer), `navbar_start/center/end/persistent` (blanked), `footer_start/end` (blanked), `use_sidenotes` (Tufte-style margin/side notes CSS).

### 5.4 JS behaviors
- Inherits all PST JS (theme switcher, scrollspy, version switcher, Ctrl+K search).
- `sphinx-book-theme.js`: primary-sidebar show/hide toggle, **fullscreen button**, header-button dropdowns (download formats, launch services), optional **Thebe** activation (live code cells via Binder) when `launch_buttons.thebe` is set.

### 5.5 Rust renderability
- Marginal cost over PST is small: the extra Python logic is URL string-building for repo/launch buttons — trivially portable to Rust. `launch_buttons`/Thebe integration can ship as static config-driven JS.
- Licensing: BSD-3-Clause. Vendorable.

### 5.6 Strategy
**Same hybrid as PST, layered on top of it.** Implement only after PST; treat it as a validation case for cross-package theme inheritance and component overriding in the theme abstraction layer.

---

## 6. Cross-cutting: the shared "Sphinx theme contract" sphinx-ultra must implement

Every theme above assumes this runtime contract; implement it once, behind a trait:

1. **Theme resolution**: locate themes from Python packages (read entry points/`theme.conf` from installed wheels or a vendored registry), parse `theme.conf` INI (all five still use it; support `theme.toml` — Sphinx ≥7.3 format — for future-proofing), resolve `inherit` chains transitively (`furo→basic-ng→basic`, `book→pydata→basic`), merge `[options]` with `html_theme_options` (reject unknown keys like Sphinx does; allow structured values).
2. **Template context**: `pathto(docname, resource=false)` (relative-URL closure), `hasdoc()`, `toctree(**kwargs)` as a callable returning globaltoc HTML (maxdepth/collapse/includehidden/titles_only semantics + `current`/`toctree-l{n}` classes), `toc` (local), `sidebars` list + dynamic includes, `css_tag`/`js_tag` renderables honoring `html_css_files`/`html_js_files` priorities, `metatags`, `next`/`prev`/`parents`, `title`/`shorttitle`/`docstitle`, `master_doc`/`root_doc`, `sourcename`/`page_source_suffix`, `last_updated`, `show_sphinx`, `language`, `favicon_url`/`logo_url`, `theme_*` flattening, full `html_context` passthrough, `embedded`/`builder` flags.
3. **Jinja dialect**: minijinja covers extends/blocks/include (dynamic names), macros, set, filters/tests, whitespace control. Must add: Sphinx filters `tobool`, `toint`; `_()`/`gettext`/`ngettext` functions backed by the theme's `locale/` catalogs; and a **preprocessing pass converting `{% trans %}…{% endtrans %}` blocks to `gettext()` calls** (minijinja has no i18n tag). Also implement the **`_t` static-file templating** pipeline (alabaster) and per-theme `pygments_style`/`pygments_dark_style` mapping to the Rust highlighter (including Python-class styles: vendor color tables for `alabaster.support.Alabaster`, `a11y-*` styles).
4. **Core JS/search compatibility**: emit Sphinx-compatible `documentation_options`, `doctools.js`, `sphinx_highlight.js`, `language_data.js`, `searchtools.js`, and a **byte-format-compatible `searchindex.js`** (docnames/filenames/terms/titleterms/alltitles/indexentries + Snowball-equivalent stemming) — all five themes' search UIs sit on top of it.
5. **Asset vendoring**: vendor from **released wheels, not git trees** (furo/PST/book-theme assets are build artifacts). Ship as embedded assets (rust-embed) or a versioned "theme pack"; record upstream version + license per pack (MIT: furo, rtd, basic-ng, jQuery, Bootstrap; BSD-3: pydata, book, alabaster, shibuya; BSD-2: Sphinx `basic`; OFL/Apache/CC-BY: fonts/icons). All are permissive — vendoring is licensing-viable across the board with a consolidated THIRD-PARTY-NOTICES file.

## 7. Overall architecture recommendation

- **Theme abstraction layer**: a `Theme` trait with two implementations:
  1. `JinjaTheme` (default path): vendored theme pack (templates + assets + theme.conf + locales) rendered via minijinja with the shared context above. Covers **alabaster, sphinx-rtd-theme, furo** with zero-to-small native shims.
  2. `JinjaTheme + NativeHelpers`: same, plus a per-theme Rust hook trait (`fn page_context(&mut Context, &PageModel)`) replicating upstream `html-page-context` Python callables. Covers **furo's nav-tree rewrite** and **pydata/book's `generate_*_html` family**.
  Avoid the third option (emit theme-compatible HTML without the theme's templates) except as a fallback: class-compatible HTML without real templates breaks user template overrides (`templates_path` + `{% extends "!page.html" %}`), which all five ecosystems rely on. Support user template overrides by putting the project's `templates_path` ahead of the vendored pack in minijinja's loader with the `!`-prefix parent-lookup convention.
- **Implementation order**: alabaster (bootstraps machinery) → sphinx-rtd-theme (biggest install base, pure Jinja) → furo (small shim, huge mindshare) → pydata-sphinx-theme (largest shim) → sphinx-book-theme (delta on PST). Shibuya is the recommended 6th (pure-Jinja, BSD, actively maintained); sphinx-immaterial explicitly out of scope for v1 (deep Python coupling, lowest fidelity-per-effort).
- **Compatibility escape hatch**: keep loading *unvendored* third-party themes best-effort through the same `JinjaTheme` path (read theme.conf from site-packages), with a warning that Python-side hooks won't run — this gracefully degrades for the long tail.

## Sources

- [pypistats.org/packages/furo](https://www.pypistats.org/packages/furo)
- [pypistats.org/packages/sphinx-rtd-theme](https://pypistats.org/packages/sphinx-rtd-theme)
- [pypistats.org/packages/pydata-sphinx-theme](https://pypistats.org/packages/pydata-sphinx-theme)
- [pypistats.org/packages/sphinx-book-theme](https://pypistats.org/packages/sphinx-book-theme)
- [pypistats.org/packages/alabaster](https://pypistats.org/packages/alabaster)
- [pypistats.org/packages/shibuya](https://pypistats.org/packages/shibuya)
- [pypistats.org/packages/sphinx-immaterial](https://pypistats.org/packages/sphinx-immaterial)
- GitHub API (stars/licenses/theme.conf contents): [pradyunsg/furo](https://github.com/pradyunsg/furo), [readthedocs/sphinx_rtd_theme](https://github.com/readthedocs/sphinx_rtd_theme), [pydata/pydata-sphinx-theme](https://github.com/pydata/pydata-sphinx-theme), [executablebooks/sphinx-book-theme](https://github.com/executablebooks/sphinx-book-theme), [sphinx-doc/alabaster](https://github.com/sphinx-doc/alabaster), [lepture/shibuya](https://github.com/lepture/shibuya), [jbms/sphinx-immaterial](https://github.com/jbms/sphinx-immaterial), [schettino72/sphinx_press_theme](https://github.com/schettino72/sphinx_press_theme)
- [Sphinx HTML theming docs](https://www.sphinx-doc.org/en/master/usage/theming.html)
- [Write the Docs: Sphinx themes](https://www.writethedocs.org/guide/tools/sphinx-themes/)
- [Shibuya: Alternatives](https://shibuya.lepture.com/alternatives/)