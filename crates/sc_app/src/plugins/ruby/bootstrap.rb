# SketchCAD extension host. SketchCAD starts this script; it is not meant to be run by hand.
#
#   ruby bootstrap.rb <plugins dir> <settings.json> <data dir>
#
# stdout carries the JSON protocol to SketchCAD, so everything extensions print (puts, p, warn)
# goes to stderr, which SketchCAD shows in the Ruby Console.

require 'json'

PROTO_OUT = STDOUT.dup
PROTO_OUT.sync = true
PROTO_IN = STDIN
STDOUT.reopen(STDERR)
STDOUT.sync = true
STDERR.sync = true
Encoding.default_external = Encoding::UTF_8

api_dir = File.dirname(File.expand_path(__FILE__))
plugins_dir, settings_path, data_dir = ARGV[0], ARGV[1], ARGV[2]
ARGV.clear

# Extensions find each other through the Plugins folder; SketchCAD's API files must win over any
# extension file that happens to share a name (sketchup.rb, extensions.rb, ...).
$LOAD_PATH.unshift(plugins_dir) if plugins_dir
$LOAD_PATH.unshift(api_dir)

require 'sketchcad_api'
require 'extensions'
require 'langhandler'
require 'sketchup'

SketchCAD::Host.start(plugins_dir, settings_path, data_dir)
