/* Basic jQuery compatibility for Sphinx */
(function(global) {
    "use strict";

    function $(selector, context) {
        if (typeof selector === 'string') {
            const elements = (context || document).querySelectorAll(selector);
            return Object.assign(Array.from(elements), $.fn);
        } else if (typeof selector === 'function') {
            if (document.readyState === 'loading') {
                document.addEventListener('DOMContentLoaded', selector);
            } else {
                selector();
            }
            return $;
        }
        return selector;
    }

    // Add basic methods
    $.fn = {
        each: function(callback) {
            for (let i = 0; i < this.length; i++) {
                callback.call(this[i], i, this[i]);
            }
            return this;
        },
        click: function(handler) {
            this.forEach(el => el.addEventListener('click', handler));
            return this;
        },
        toggle: function() {
            this.forEach(el => {
                el.style.display = el.style.display === 'none' ? '' : 'none';
            });
            return this;
        },
        html: function(content) {
            if (content !== undefined) {
                this.forEach(el => el.innerHTML = content);
                return this;
            }
            return this[0] ? this[0].innerHTML : '';
        },
        find: function(selector) {
            const results = [];
            this.forEach(el => {
                const found = el.querySelectorAll(selector);
                results.push(...found);
            });
            return Object.assign(results, $.fn);
        }
    };

    // Add ready function
    $.ready = function(callback) {
        return $(callback);
    };

    global.$ = global.jQuery = $;
})(window);
