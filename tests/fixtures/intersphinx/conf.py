project = 'Intersphinx Fixture'

# A local inventory file: read from the source directory on every build, so
# this project never needs the network. The target URI is where the links
# point, and is deliberately unrelated to where the inventory was read from.
intersphinx_mapping = {
    'other': ('https://example.org/', 'local.inv'),
}
