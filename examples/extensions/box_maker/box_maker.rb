# Box Maker — an example SketchUp extension, written exactly as it would be for SketchUp.
# Loader file: registers the extension; the code lives in the box_maker/ folder.

require 'sketchup.rb'
require 'extensions.rb'

module Examples
  module BoxMaker
    unless file_loaded?(__FILE__)
      ex = SketchupExtension.new('Box Maker', 'box_maker/main')
      ex.description = 'Draws a box as a group from a width, depth and height.'
      ex.version     = '1.0.0'
      ex.creator     = 'SketchCAD examples'
      ex.copyright   = 'Public domain'
      Sketchup.register_extension(ex, true)
      file_loaded(__FILE__)
    end
  end
end
