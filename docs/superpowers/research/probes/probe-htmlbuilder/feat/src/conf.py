project = 'Feat & "Q"'
copyright = ['2020, A <b>', '2021, B']
release = '2.0'
version = '2.0'
html_theme = 'basic'
html_last_updated_fmt = '%Y'
html_logo = '_static/logo.png'
html_favicon = '_static/fav.ico'
html_baseurl = 'https://example.org/docs/'
html_use_opensearch = 'https://example.org/docs'
html_static_path = ['_static']
html_css_files = ['custom.css', ('print.css', {'media': 'print', 'priority': 100}), 'https://cdn.example.org/x.css']
html_js_files = ['custom.js', ('defer.js', {'defer': 'defer', 'async': 'async'}), (None, {'body': 'var x = 1 < 2;'})]
html_split_index = True
html_sourcelink_suffix = ''
html_context = {'extra_ctx': 'yes'}
