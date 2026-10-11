# Helpers SketchUp's own sketchup.rb provides to every extension.

require 'sketchcad_api'
require 'extensions'
require 'langhandler'

$loaded_files ||= []

def file_loaded?(name)
  $loaded_files.include?(name.to_s.downcase)
end

def file_loaded(name)
  $loaded_files << name.to_s.downcase unless file_loaded?(name)
end

def inputbox(*args)
  UI.inputbox(*args)
end

def add_separator_to_menu(name)
  UI.menu(name).add_separator
end

def require_all(dir)
  Dir.glob(File.join(dir, '*.rb')).sort.each { |f| Sketchup.require(f) }
end

def show_ruby_panel
  nil
end
