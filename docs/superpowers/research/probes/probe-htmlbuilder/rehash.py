import hashlib
def md5(s): return hashlib.md5(s.encode()).hexdigest()
def py_str(v):
    # the leaf string form
    return str(v)
def h(v):
    if isinstance(v, dict):
        items = sorted(h_tuple(k, x) for k, x in v.items())
        return md5(str(sorted(md5(i) for i in items)))  # double-hash
    if isinstance(v, (list, tuple, set, frozenset)):
        return md5(str(sorted(h(x) for x in v)))
    return md5(py_str(v))
def h_tuple(k, x):
    return md5(str(sorted([h(k), h(x)])))
vals = {
'copyright': '2026, Tester','html4_writer': False,'html_additional_pages': {},'html_baseurl': '','html_codeblock_linenos_style': 'inline','html_compact_lists': True,'html_context': {},'html_copy_source': True,'html_css_files': [],'html_domain_indices': True,'html_extra_path': [],'html_favicon': None,'html_file_suffix': None,'html_js_files': [],'html_last_updated_fmt': None,'html_last_updated_use_utc': False,'html_link_suffix': None,'html_logo': None,'html_output_encoding': 'utf-8','html_permalinks': True,'html_permalinks_icon': '¶','html_scaled_image_link': True,'html_search_language': None,'html_search_options': {},'html_secnumber_suffix': '. ','html_short_title': 'Probe 1.0 documentation','html_show_copyright': True,'html_show_search_summary': True,'html_show_sourcelink': True,'html_show_sphinx': True,'html_sidebars': {},'html_sourcelink_suffix': '.txt','html_split_index': False,'html_static_path': [],'html_style': None,'html_theme': 'basic','html_theme_options': {},'html_theme_path': [],'html_title': 'Probe 1.0 documentation','html_use_index': True,'html_use_opensearch': '','htmlhelp_file_suffix': None,'htmlhelp_link_suffix': None,'mathjax2_config': None,'mathjax3_config': None,'mathjax4_config': None,'mathjax_config': None,'mathjax_config_path': '','mathjax_display': ['\\[', '\\]'],'mathjax_inline': ['\\(', '\\)'],'mathjax_options': {},'mathjax_path': 'https://cdn.jsdelivr.net/npm/mathjax@4/tex-mml-chtml.js','modindex_common_prefix': [],'project_copyright': '2026, Tester','pygments_style': None,'qthelp_basename': 'Probe','qthelp_namespace': None,'qthelp_theme': 'nonav','qthelp_theme_options': {},'singlehtml_sidebars': {},'template_bridge': None,'templates_path': []}
print(h(vals), 'expected ee8c8458af746a6d4f6c98626c0842b6')
print(h(['builder_html','format_html','html']))
print(h({'a': {'b': 1}}))
