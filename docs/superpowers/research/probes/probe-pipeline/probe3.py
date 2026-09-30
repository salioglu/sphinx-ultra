import io, sys, tempfile, pathlib
from sphinx.application import Sphinx
from sphinx.builders.html._build_info import BuildInfo
from sphinx.util._serialise import stable_hash
d = pathlib.Path(tempfile.mkdtemp())
(d/'src').mkdir(); (d/'src'/'conf.py').write_text("project = 'P'\n"); (d/'src'/'index.rst').write_text("I\n=\n")
app = Sphinx(d/'src', d/'src', d/'out', d/'out'/'.doctrees', 'html', status=io.StringIO(), warning=io.StringIO())
vals = {c.name: c.value for c in app.config.filter(frozenset({'html'}))}
for k in sorted(vals): print(f"{k} = {vals[k]!r}")
print("CONFIG_HASH", stable_hash(vals))
print("TAGS", sorted(app.tags), stable_hash(sorted(app.tags)))
print("---- env category keys:")
print(sorted(c.name for c in app.config.filter(frozenset({'env'}))))
print("---- selected defaults")
for k in ['project','version','release','copyright','language','today','today_fmt','html_theme','html_title','html_short_title','html_static_path','templates_path','html_last_updated_fmt','html_permalinks','html_permalinks_icon','html_baseurl','html_file_suffix','html_link_suffix','html_domain_indices','html_use_index','html_split_index','html_secnumber_suffix','html_compact_lists','html_codeblock_linenos_style','html_math_renderer','html_scaled_image_link','html_show_copyright','html_show_sphinx','html_output_encoding','html_style','pygments_style','highlight_language','html_sidebars','html_context','html_copy_source','html_show_sourcelink','html_sourcelink_suffix','html_use_opensearch','html_extra_path','html_css_files','html_js_files','html_logo','html_favicon','html_theme_options','html_additional_pages','html_show_search_summary','html_search_language','keep_warnings','root_doc','author','html_last_updated_use_utc']:
    try:
        c = app.config._options[k]
        print(f"{k}: default={app.config[k]!r} rebuild={c.rebuild!r} types={c.valid_types!r}")
    except Exception as e:
        print(k, "ERR", e)
