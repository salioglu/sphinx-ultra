/* Basic Sphinx highlight functionality */
window.SphinxHighlight = {
    highlight: function(term) {
        if (!term) return;

        var regex = new RegExp('(' + term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + ')', 'gi');
        var content = document.querySelector('.rst-content');

        if (content) {
            var walker = document.createTreeWalker(
                content,
                NodeFilter.SHOW_TEXT,
                null,
                false
            );

            var textNodes = [];
            var node;

            while (node = walker.nextNode()) {
                textNodes.push(node);
            }

            textNodes.forEach(function(textNode) {
                if (regex.test(textNode.textContent)) {
                    var span = document.createElement('span');
                    span.innerHTML = textNode.textContent.replace(regex, '<mark>$1</mark>');
                    textNode.parentNode.replaceChild(span, textNode);
                }
            });
        }
    }
};
