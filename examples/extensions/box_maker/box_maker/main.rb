module Examples
  module BoxMaker
    # Draw a box in the active context, as one undoable operation.
    def self.create_box(width, depth, height)
      model = Sketchup.active_model
      model.start_operation('Create Box', true)
      group = model.active_entities.add_group
      group.name = 'Box'
      face = group.entities.add_face([0, 0, 0], [width, 0, 0], [width, depth, 0], [0, depth, 0])
      face.reverse! if face.normal.z < 0
      face.pushpull(height)
      model.commit_operation
      model.selection.clear
      model.selection.add(group)
      group
    end

    def self.prompt_box
      prompts = ['Width', 'Depth', 'Height']
      defaults = [1000.mm, 600.mm, 800.mm]
      input = UI.inputbox(prompts, defaults, 'Box Maker')
      return unless input
      create_box(*input)
    end

    def self.count_faces
      faces = Sketchup.active_model.active_entities.grep(Sketchup::Face).length
      UI.messagebox("There are #{faces} faces in the current context.")
    end

    unless file_loaded?(__FILE__)
      menu = UI.menu('Extensions').add_submenu('Box Maker')
      menu.add_item('Create Box...') { prompt_box }
      menu.add_separator
      menu.add_item('Count Faces') { count_faces }

      cmd = UI::Command.new('Create Box') { prompt_box }
      cmd.tooltip = 'Create Box'
      cmd.status_bar_text = 'Draw a box from width, depth and height.'
      cmd.small_icon = cmd.large_icon = File.join(__dir__, 'icons', 'box.png')
      toolbar = UI::Toolbar.new('Box Maker')
      toolbar.add_item(cmd)
      toolbar.show

      file_loaded(__FILE__)
    end
  end
end
