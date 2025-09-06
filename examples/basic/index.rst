Welcome to Sphinx Ultra Basic Example
====================================

This is a basic example project demonstrating the core features of Sphinx Ultra.

Getting Started
---------------

Sphinx Ultra is a high-performance Rust-based documentation builder that provides:

* **Fast builds** - Parallel processing for maximum speed
* **Live reload** - Instant preview of changes during development
* **Modern themes** - Beautiful, responsive documentation
* **Easy configuration** - Simple YAML-based setup

Features
--------

Performance
~~~~~~~~~~~

Sphinx Ultra is designed for speed:

.. code-block:: bash

   # Build 1000 files in under 10 seconds
   sphinx-ultra build --source . --output _build

Configuration
~~~~~~~~~~~~~

Simple YAML configuration:

.. code-block:: yaml

   source_dir: "."
   output_dir: "_build"
   theme:
     name: "sphinx_rtd_theme"

Usage Examples
--------------

Basic Commands
~~~~~~~~~~~~~~

Build documentation:

.. code-block:: bash

   sphinx-ultra build --source . --output _build

Start development server:

.. code-block:: bash

   sphinx-ultra serve --source . --port 8000

Advanced Usage
~~~~~~~~~~~~~~

With custom configuration:

.. code-block:: bash

   sphinx-ultra build --config custom.yaml --jobs 8

API Reference
-------------

For detailed API documentation, see :doc:`api`.

.. toctree::
   :maxdepth: 2
   :caption: Contents:

   getting-started
   configuration
   api
   examples

Indices and tables
==================

* :ref:`genindex`
* :ref:`modindex`
* :ref:`search`
