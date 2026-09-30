from sphinx.util.tags import Tags
t = Tags(['html', 'format_html', 'builder_html'])
for e in ['html', 'not html', 'html and latex', 'html or latex', '(html or latex) and not epub', 'html if latex else epub', 'html and', 'html && latex', '(html', 'True', 'html latex', 'html,', 'builder_html', 'html-x', '1', "'html'", 'html == latex']:
    try:
        print(repr(e), '->', t.eval_condition(e))
    except Exception as err:
        print(repr(e), '-> EXC', type(err).__name__, repr(str(err)))
