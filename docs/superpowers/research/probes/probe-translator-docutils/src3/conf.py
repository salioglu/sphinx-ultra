project = 'P3'
html_theme = 'basic'
def _dump(app, pagename, templatename, ctx, doctree):
    if doctree is None: return
    s = app.builder.docsettings
    keys = ['initial_header_level','footnote_references','attribution','compact_lists','compact_field_lists',
            'table_style','math_output','xml_declaration','cloak_email_addresses','embed_stylesheet','image_loading',
            'section_self_link','toc_backlinks','footnote_backlinks','report_level','halt_level','language_code',
            'output_encoding','file_insertion_enabled','strict_visitor','embed_images','sectnum_xform','strip_comments',
            'doctitle_xform','sectsubtitle_xform','trim_footnote_reference_space','auto_id_prefix','id_prefix',
            'stylesheet_path','stylesheet','template','root_prefix','output_path','warning_stream','traceback','smart_quotes',
            'field_name_limit','option_limit','record_dependencies','source_link','datestamp','generator']
    with open(app.outdir + '/../settings3.txt', 'w') as f:
        for k in keys:
            f.write(f'{k} = {getattr(s, k, "<unset>")!r}\n')
        # translator initial state
        from sphinx.writers.html5 import HTML5Translator
        t = app.builder.create_translator(doctree, app.builder)
        f.write(f'translator.initial_header_level = {t.initial_header_level!r}\n')
        f.write(f'translator.math_output = {t.math_output!r} {t.math_options!r}\n')
        f.write(f'translator.image_loading = {t.image_loading!r}\n')
        f.write(f'translator.meta = {t.meta!r}\n')
        f.write(f'translator.body_prefix = {t.body_prefix!r}\n')
        f.write(f'translator class MRO = {[k.__module__+"."+k.__name__ for k in type(t).__mro__]}\n')
def setup(app):
    app.connect('html-page-context', _dump)
