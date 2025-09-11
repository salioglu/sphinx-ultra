/* Sphinx-compatible JavaScript - basic functionality */
$(document).ready(function() {
    // Basic functionality for Sphinx compatibility
    console.log("Sphinx Ultra Builder - JavaScript loaded");

    // Highlight search terms if present
    if (window.location.search) {
        var params = new URLSearchParams(window.location.search);
        var searchTerm = params.get('q');
        if (searchTerm) {
            highlightSearchTerms(searchTerm);
        }
    }

    // Mobile nav toggle
    $('[data-toggle="wy-nav-top"]').click(function() {
        $('.wy-nav-side').toggle();
    });
});

function highlightSearchTerms(term) {
    if (!term) return;

    var content = $('.rst-content');
    var regex = new RegExp('(' + term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + ')', 'gi');

    content.find('p, li, td, th').each(function() {
        var $this = $(this);
        var html = $this.html();
        if (html && html.indexOf('<') === -1) { // Only process text nodes
            $this.html(html.replace(regex, '<mark>$1</mark>'));
        }
    });
}
