# LanguageHandler: SketchCAD is English-only, so strings translate to themselves.

class LanguageHandler
  def initialize(_file = nil); @strings = {}; end
  def GetString(s); @strings[s] || s; end # rubocop:disable Naming/MethodName
  def [](s); GetString(s); end
  def strings; @strings; end
  def resource_path(_f = nil); ''; end
end
