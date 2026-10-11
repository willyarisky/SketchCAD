# SketchupExtension: how extensions register themselves (SketchCAD version of SketchUp's file).

require 'sketchcad_api'

class SketchupExtension
  attr_accessor :name, :description, :version, :creator, :copyright, :extension_path, :id, :version_id
  attr_reader :path

  def initialize(name, path)
    @name = name.to_s
    @path = path.to_s
    @description = ''
    @version = ''
    @creator = ''
    @copyright = ''
    @loaded = false
    @registered = false
    @load_on_start = false
  end

  def loaded?; @loaded; end
  def registered?; @registered; end
  def load_on_start?; @load_on_start; end

  # Load the extension now.
  def check
    return true if @loaded
    SketchCAD.require_file(@path)
    @loaded = true
    @load_on_start = true
    SketchCAD::Host.note('ext.loaded', 'name' => @name)
    true
  rescue Exception => e # rubocop:disable Lint/RescueException
    raise if e.is_a?(SystemExit)
    STDERR.puts("Error loading extension #{@name}: #{e.class}: #{e.message}")
    (e.backtrace || []).reject { |l| l.start_with?(SketchCAD::API_DIR) }[0, 6].each { |l| STDERR.puts("    #{l}") }
    false
  end

  # Turning an extension off takes effect when Ruby restarts (SketchCAD does that for you).
  def uncheck
    @load_on_start = false
    false
  end

  def _register(load_on_start, loader)
    @registered = true
    disabled = (SketchCAD::Host.disabled || []).include?(@name)
    enabled = load_on_start && !disabled
    abs = File.expand_path(@path.sub(/\.(rb|rbe|rbs)\z/i, ''), SketchCAD::Host.plugins_dir || Dir.pwd)
    Sketchup.extensions._add(self)
    SketchCAD::Host.note('ext.register', 'name' => @name, 'version' => @version.to_s, 'creator' => @creator.to_s,
                                         'description' => @description.to_s, 'copyright' => @copyright.to_s,
                                         'path' => abs, 'loader' => loader.to_s, 'enabled' => enabled)
    check if enabled
  end
end
