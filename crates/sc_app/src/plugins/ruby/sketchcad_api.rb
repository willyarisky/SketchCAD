# SketchCAD's implementation of the SketchUp Ruby API.
#
# Geometry lives in SketchCAD, not here: entities are thin handles and every query or change is a
# request to SketchCAD. Geom (points, vectors, transformations, bounding boxes) runs in Ruby.
# Units are inches internally, as in SketchUp. Compatible with Ruby 2.6 and later.

require 'json'
require 'tmpdir'

module SketchCAD
  API_DIR = File.dirname(File.expand_path(__FILE__))

  # Protocol and job loop.
  module Host
    @next_id = 0
    @procs = {}
    @next_proc = 0
    @warned = {}
    @objects = {}

    class << self
      attr_reader :plugins_dir, :data_dir, :disabled, :current_loader

      def send_msg(h)
        PROTO_OUT.write(JSON.generate(h) + "\n")
      end

      def read_msg
        line = PROTO_IN.gets
        line && JSON.parse(line)
      end

      # Ask SketchCAD something and wait for the answer.
      def request(m, *args)
        @next_id += 1
        id = @next_id
        send_msg('t' => 'req', 'id' => id, 'm' => m, 'a' => args)
        loop do
          msg = read_msg
          exit!(0) if msg.nil? || msg['t'] == 'quit'
          next unless msg['t'] == 'reply' && msg['id'] == id
          if msg.key?('err')
            err = msg['err'].to_s
            raise TypeError, err if err.start_with?('reference to deleted')
            raise ArgumentError, err
          end
          return msg['ok']
        end
      end

      def note(m, h = {})
        send_msg('t' => 'note', 'm' => m, 'a' => h)
      end

      def register_proc(blk)
        @next_proc += 1
        @procs[@next_proc] = blk
        @next_proc
      end

      def warn_once(key, text)
        return if @warned[key]
        @warned[key] = true
        STDERR.puts("SketchCAD: #{text}")
      end

      # The single Ruby object for an entity handle (SketchUp returns the same object each time).
      def wrap(h)
        return nil if h.nil?
        @objects[h] ||= case h[0]
                        when 'E' then Sketchup::Edge.new(h)
                        when 'F' then Sketchup::Face.new(h)
                        when 'V' then Sketchup::Vertex.new(h)
                        when 'G' then Sketchup::Group.new(h)
                        when 'C' then Sketchup::ComponentInstance.new(h)
                        when 'D' then Sketchup::ComponentDefinition.new(h)
                        else raise ArgumentError, "unknown entity #{h}"
                        end
      end

      def start(plugins_dir, settings_path, data_dir)
        @plugins_dir = plugins_dir
        @data_dir = data_dir
        settings = begin
          JSON.parse(File.read(settings_path))
        rescue StandardError
          {}
        end
        @disabled = settings['disabled'] || []
        if plugins_dir && File.directory?(plugins_dir)
          Dir.glob(File.join(plugins_dir, '*.{rbe,rbs}')).sort.each do |f|
            STDERR.puts("Skipped #{File.basename(f)}: encrypted extensions only run inside SketchUp.")
          end
          Dir.glob(File.join(plugins_dir, '*.rb')).sort.each { |f| load_plugin(f) }
        end
        send_msg('t' => 'ready')
        main_loop
      end

      # Load an extension's top-level .rb file, reporting (not raising) its errors.
      def load_plugin(path)
        @current_loader = path
        require(path)
      rescue Exception => e # rubocop:disable Lint/RescueException
        raise if e.is_a?(SystemExit)
        STDERR.puts("Error loading #{File.basename(path)}: #{e.class}: #{e.message}")
        (e.backtrace || [])[0, 6].each { |l| STDERR.puts("    #{l}") }
      ensure
        @current_loader = nil
      end

      def main_loop
        while (msg = read_msg)
          case msg['t']
          when 'eval'
            run_job(msg['id']) do
              r = TOPLEVEL_BINDING.eval(msg['code'], '(eval)')
              s = r.inspect
              s.length > 4000 ? s[0, 4000] + '...' : s
            end
          when 'call'
            run_job(msg['id']) do
              blk = @procs[msg['proc']]
              raise 'this command no longer exists' unless blk
              blk.call
              nil
            end
          when 'load'
            run_job(msg['id']) do
              @current_loader = msg['path']
              begin
                require(msg['path'])
              ensure
                @current_loader = nil
              end
              nil
            end
          when 'quit'
            break
          end
        end
      end

      def run_job(id)
        result = yield
        send_msg('t' => 'done', 'id' => id, 'ok' => true, 'result' => result)
      rescue Exception => e # rubocop:disable Lint/RescueException
        raise if e.is_a?(SystemExit)
        bt = (e.backtrace || []).reject { |l| l.start_with?(API_DIR) }[0, 8]
        send_msg('t' => 'done', 'id' => id, 'ok' => false, 'err' => "#{e.class}: #{e.message}", 'bt' => bt)
      end
    end
  end

  # Persisted per-session only: SketchCAD files do not store attribute dictionaries yet.
  ATTRIBUTES = Hash.new { |h, k| h[k] = {} }

  def self.pt(o)
    case o
    when Geom::Point3d, Geom::Vector3d then [o.x.to_f, o.y.to_f, o.z.to_f]
    when Sketchup::Vertex then pt(o.position)
    when Array then [o[0].to_f, o[1].to_f, (o[2] || 0).to_f]
    else raise TypeError, "cannot convert #{o.class} into a 3D point"
    end
  end

  def self.vec(o)
    Geom::Vector3d.new(*pt(o))
  end

  def self.handles(list)
    list.flatten.map do |e|
      raise TypeError, "expected an entity, got #{e.class}" unless e.respond_to?(:_handle)
      e._handle
    end
  end

  # Points of a list given as (p1, p2, ...) or ([p1, p2, ...]).
  def self.point_list(args)
    args = args[0] if args.length == 1 && args[0].is_a?(Array) && !args[0][0].is_a?(Numeric)
    args.map { |p| pt(p) }
  end

  # Ordered loop points of connected edges.
  def self.loop_points(edges)
    edges = edges.flat_map { |e| e.is_a?(Sketchup::Curve) ? e.edges : [e] }
    return [] if edges.empty?
    rest = edges.dup
    first = rest.shift
    pts = [first.start]
    cur = first.end
    until rest.empty?
      i = rest.index { |e| e.start == cur || e.end == cur }
      break unless i
      e = rest.delete_at(i)
      pts << cur
      cur = e.other_vertex(cur)
    end
    pts << cur unless cur == pts[0]
    pts.map { |v| v.position.to_a }
  end

  def self.color_value(m)
    case m
    when nil then nil
    when Sketchup::Material then m.color.to_a
    when Sketchup::Color then m.to_a
    when String
      mat = Sketchup.active_model.materials[m]
      mat ? mat.color.to_a : Sketchup::Color.new(m).to_a
    when Array then Sketchup::Color.new(m).to_a
    else raise TypeError, "cannot use #{m.class} as a material"
    end
  end

  def self.material_for(rgba)
    return nil if rgba.nil?
    Sketchup.active_model.materials.for_color(Sketchup::Color.new(rgba))
  end

  # require that understands SketchUp extension paths (no extension, encrypted variants).
  def self.require_file(path)
    base = path.to_s.sub(/\.(rb|rbe|rbs)\z/i, '')
    abs = File.expand_path(base, Host.plugins_dir || Dir.pwd)
    return require(abs + '.rb') if File.exist?(abs + '.rb')
    if File.exist?(abs + '.rbe') || File.exist?(abs + '.rbs')
      raise LoadError, "#{File.basename(abs)} is encrypted (.rbe/.rbs); encrypted extensions only run inside SketchUp"
    end
    return require(abs) if File.file?(abs)
    require(path.to_s)
  end

  # A length in the model's display units, e.g. "1000mm" or "3.5'".
  def self.format_length(v)
    unit = Sketchup.active_model._info['unit']
    val, suffix = case unit
                  when 'Millimeters' then [v.to_mm, 'mm']
                  when 'Centimeters' then [v.to_cm, 'cm']
                  when 'Meters' then [v.to_m, 'm']
                  when 'Feet' then [v.to_feet, "'"]
                  else [v.to_f, '"']
                  end
    format('%.4f', val).sub(/\.?0+\z/, '') + suffix
  end

  def self.prefs_path
    File.join(Host.data_dir || Dir.tmpdir, 'ruby_defaults.json')
  end

  def self.prefs
    JSON.parse(File.read(prefs_path))
  rescue StandardError
    {}
  end
end

# ---------------------------------------------------------------------------
# Units

class Numeric
  def inch; to_f; end
  def feet; to_f * 12.0; end
  def yard; to_f * 36.0; end
  def mile; to_f * 63_360.0; end
  def mm; to_f / 25.4; end
  def cm; to_f / 2.54; end
  def m; to_f / 0.0254; end
  def km; to_f / 0.0000254; end
  def to_inch; to_f; end
  def to_feet; to_f / 12.0; end
  def to_yard; to_f / 36.0; end
  def to_mile; to_f / 63_360.0; end
  def to_mm; to_f * 25.4; end
  def to_cm; to_f * 2.54; end
  def to_m; to_f * 0.0254; end
  def to_km; to_f * 0.0000254; end
  def degrees; to_f * Math::PI / 180.0; end
  def radians; to_f * 180.0 / Math::PI; end
  def to_l; to_f; end
end

# SketchUp's Length is a Float in inches.
Length = Float unless defined?(Length)
class Float
  Inches = 0 unless const_defined?(:Inches, false)
  Feet = 1 unless const_defined?(:Feet, false)
  Millimeter = 2 unless const_defined?(:Millimeter, false)
  Centimeter = 3 unless const_defined?(:Centimeter, false)
  Meter = 4 unless const_defined?(:Meter, false)
  Decimal = 0 unless const_defined?(:Decimal, false)
  Architectural = 1 unless const_defined?(:Architectural, false)
  Engineering = 2 unless const_defined?(:Engineering, false)
  Fractional = 3 unless const_defined?(:Fractional, false)
end

class String
  # "10mm", "2.5m", "3'6\"", "12" (inches).
  def to_l
    s = strip
    if (m = s.match(/\A(-?[\d.]+)'\s*(?:-?\s*([\d.]+)"?)?\z/))
      return m[1].to_f * 12.0 + m[2].to_f
    end
    m = s.match(/\A(-?[\d.]+(?:e-?\d+)?)\s*(mm|cm|m|km|"|in|inch|inches|ft|feet|'|yd)?\z/i)
    raise ArgumentError, "Cannot convert \"#{self}\" to Length" unless m
    v = m[1].to_f
    case (m[2] || '').downcase
    when 'mm' then v.mm
    when 'cm' then v.cm
    when 'm' then v.m
    when 'km' then v.km
    when 'ft', 'feet', "'" then v.feet
    when 'yd' then v.yard
    else v
    end
  end
end

# ---------------------------------------------------------------------------
# Geom

module Geom
  class Point3d
    attr_accessor :x, :y, :z

    def initialize(*a)
      if a.length == 1 && (a[0].is_a?(Point3d) || a[0].is_a?(Vector3d))
        @x, @y, @z = a[0].x.to_f, a[0].y.to_f, a[0].z.to_f
      else
        a = a[0] if a.length == 1 && a[0].is_a?(Array)
        @x, @y, @z = (a[0] || 0).to_f, (a[1] || 0).to_f, (a[2] || 0).to_f
      end
    end

    def self.linear_combination(w1, p1, w2, p2)
      a, b = Point3d.new(p1), Point3d.new(p2)
      Point3d.new(a.x * w1 + b.x * w2, a.y * w1 + b.y * w2, a.z * w1 + b.z * w2)
    end

    def to_a; [@x, @y, @z]; end
    def [](i); to_a[i]; end
    def []=(i, v); i.zero? ? @x = v.to_f : (i == 1 ? @y = v.to_f : @z = v.to_f); end
    def clone; Point3d.new(@x, @y, @z); end
    alias dup clone

    def set!(*a)
      p = Point3d.new(*a)
      @x, @y, @z = p.x, p.y, p.z
      self
    end

    def +(v)
      v = SketchCAD.pt(v)
      Point3d.new(@x + v[0], @y + v[1], @z + v[2])
    end

    def -(o)
      if o.is_a?(Vector3d)
        Point3d.new(@x - o.x, @y - o.y, @z - o.z)
      else
        p = SketchCAD.pt(o)
        Vector3d.new(@x - p[0], @y - p[1], @z - p[2])
      end
    end

    def ==(o)
      return false unless o.is_a?(Point3d) || o.is_a?(Array) || o.is_a?(Vector3d)
      distance(o) < 0.001
    end

    def distance(o)
      p = SketchCAD.pt(o)
      Math.sqrt((@x - p[0])**2 + (@y - p[1])**2 + (@z - p[2])**2)
    end

    def vector_to(o)
      Point3d.new(o) - self
    end

    def offset(v, len = nil)
      v = SketchCAD.vec(v)
      v = v.normalize.scale(len) if len
      self + v
    end

    def offset!(v, len = nil); set!(offset(v, len)); end
    def transform(t); Transformation.new(t) * self; end
    def transform!(t); set!(transform(t)); end
    def project_to_plane(plane); Geom.project_to_plane(self, plane); end
    def distance_to_plane(plane); distance(project_to_plane(plane)); end
    def project_to_line(line); Geom.project_to_line(self, line); end
    def distance_to_line(line); distance(project_to_line(line)); end
    def on_line?(line); distance_to_line(line) < 0.001; end
    def on_plane?(plane); distance_to_plane(plane) < 0.001; end
    def valid?; true; end
    def to_s; "(#{@x.to_mm.round(4)}mm, #{@y.to_mm.round(4)}mm, #{@z.to_mm.round(4)}mm)"; end
    def inspect; "Point3d(#{@x}, #{@y}, #{@z})"; end
  end

  class Vector3d
    attr_accessor :x, :y, :z

    def initialize(*a)
      if a.length == 1 && (a[0].is_a?(Point3d) || a[0].is_a?(Vector3d))
        @x, @y, @z = a[0].x.to_f, a[0].y.to_f, a[0].z.to_f
      else
        a = a[0] if a.length == 1 && a[0].is_a?(Array)
        @x, @y, @z = (a[0] || 0).to_f, (a[1] || 0).to_f, (a[2] || 0).to_f
      end
    end

    def self.linear_combination(*a)
      out = Vector3d.new
      a.each_slice(2) do |w, v|
        v = SketchCAD.vec(v)
        out = out + v.scale(w)
      end
      out
    end

    def to_a; [@x, @y, @z]; end
    def [](i); to_a[i]; end
    def clone; Vector3d.new(@x, @y, @z); end
    alias dup clone

    def set!(*a)
      v = Vector3d.new(*a)
      @x, @y, @z = v.x, v.y, v.z
      self
    end

    def +(o); o = SketchCAD.pt(o); Vector3d.new(@x + o[0], @y + o[1], @z + o[2]); end
    def -(o); o = SketchCAD.pt(o); Vector3d.new(@x - o[0], @y - o[1], @z - o[2]); end
    def -@; reverse; end
    def scale(s); Vector3d.new(@x * s, @y * s, @z * s); end

    # SketchUp: `*` is the cross product (a number scales, as a convenience).
    def *(o)
      return scale(o) if o.is_a?(Numeric)
      cross(o)
    end

    def cross(o)
      o = SketchCAD.pt(o)
      Vector3d.new(@y * o[2] - @z * o[1], @z * o[0] - @x * o[2], @x * o[1] - @y * o[0])
    end

    def dot(o); o = SketchCAD.pt(o); @x * o[0] + @y * o[1] + @z * o[2]; end
    alias % dot

    def length; Math.sqrt(@x * @x + @y * @y + @z * @z); end
    def length=(l); set!(normalize.scale(l)); end
    def valid?; length > 1e-10; end
    def unitvector?; (length - 1.0).abs < 1e-6; end

    def normalize
      l = length
      raise ArgumentError, 'Cannot normalize a zero-length vector' if l < 1e-12
      Vector3d.new(@x / l, @y / l, @z / l)
    end

    def normalize!; set!(normalize); end
    def reverse; Vector3d.new(-@x, -@y, -@z); end
    def reverse!; set!(reverse); end
    def parallel?(o); cross(o).length < 1e-6 * length * SketchCAD.vec(o).length; end
    def perpendicular?(o); dot(o).abs < 1e-6 * length * SketchCAD.vec(o).length; end
    def samedirection?(o); parallel?(o) && dot(o) > 0; end

    def angle_between(o)
      o = SketchCAD.vec(o)
      c = dot(o) / (length * o.length)
      Math.acos([[c, 1.0].min, -1.0].max)
    end

    # SketchUp's arbitrary axis algorithm.
    def axes
      z = normalize
      x = if z.x.abs < 1.0 / 64 && z.y.abs < 1.0 / 64
            Vector3d.new(0, 1, 0).cross(z).normalize
          else
            Vector3d.new(0, 0, 1).cross(z).normalize
          end
      [x, z.cross(x).normalize, z]
    end

    def transform(t); Transformation.new(t) * self; end
    def transform!(t); set!(transform(t)); end
    def ==(o); (o.is_a?(Vector3d) || o.is_a?(Array)) && (self - o).length < 1e-6; end
    def to_s; "(#{@x}, #{@y}, #{@z})"; end
    def inspect; "Vector3d(#{@x}, #{@y}, #{@z})"; end
  end

  # 4x4 matrix, column-major like SketchUp's to_a.
  class Transformation
    IDENTITY_A = [1.0, 0, 0, 0, 0, 1.0, 0, 0, 0, 0, 1.0, 0, 0, 0, 0, 1.0].map(&:to_f).freeze

    def initialize(*a)
      @m = IDENTITY_A.dup
      case a.length
      when 0 then nil
      when 1
        x = a[0]
        if x.is_a?(Transformation) then @m = x.to_a
        elsif x.is_a?(Array) && x.length == 16 then @m = x.map(&:to_f)
        elsif x.is_a?(Numeric) then @m = Transformation.scaling(x).to_a
        else
          p = SketchCAD.pt(x)
          @m[12], @m[13], @m[14] = p
        end
      when 2
        xa, ya, za = SketchCAD.vec(a[1]).axes
        @m = Transformation.axes(a[0], xa, ya, za).to_a
      when 3 then @m = Transformation.rotation(*a).to_a
      when 4 then @m = Transformation.axes(a[3], a[0], a[1], a[2]).to_a
      else raise ArgumentError, 'wrong number of arguments for Transformation.new'
      end
    end

    def self.from_a(m)
      t = allocate
      t.instance_variable_set(:@m, m.map(&:to_f))
      t
    end

    def self.translation(v)
      Transformation.new(Point3d.new(SketchCAD.pt(v)))
    end

    def self.rotation(point, axis, angle)
      p = SketchCAD.pt(point)
      u = SketchCAD.vec(axis).normalize
      c, s = Math.cos(angle), Math.sin(angle)
      t = 1 - c
      x, y, z = u.x, u.y, u.z
      r = [[t * x * x + c, t * x * y - s * z, t * x * z + s * y],
           [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
           [t * x * z - s * y, t * y * z + s * x, t * z * z + c]]
      m = [r[0][0], r[1][0], r[2][0], 0, r[0][1], r[1][1], r[2][1], 0, r[0][2], r[1][2], r[2][2], 0, 0, 0, 0, 1]
      rp = (0..2).map { |i| r[i][0] * p[0] + r[i][1] * p[1] + r[i][2] * p[2] }
      m[12], m[13], m[14] = p[0] - rp[0], p[1] - rp[1], p[2] - rp[2]
      from_a(m)
    end

    def self.scaling(*a)
      origin = [0.0, 0.0, 0.0]
      origin = SketchCAD.pt(a.shift) unless a[0].is_a?(Numeric)
      sx, sy, sz = a.length == 1 ? [a[0], a[0], a[0]] : a
      m = [sx, 0, 0, 0, 0, sy, 0, 0, 0, 0, sz, 0, 0, 0, 0, 1].map(&:to_f)
      m[12] = origin[0] * (1 - sx)
      m[13] = origin[1] * (1 - sy)
      m[14] = origin[2] * (1 - sz)
      from_a(m)
    end

    def self.axes(origin, xaxis, yaxis = nil, zaxis = nil)
      o = SketchCAD.pt(origin)
      x = SketchCAD.pt(xaxis)
      y = yaxis ? SketchCAD.pt(yaxis) : SketchCAD.vec(zaxis).cross(x).to_a
      z = zaxis ? SketchCAD.pt(zaxis) : SketchCAD.vec(x).cross(y).to_a
      from_a(x + [0] + y + [0] + z + [0] + o + [1])
    end

    def self.interpolate(t1, t2, w)
      a, b = Transformation.new(t1).to_a, Transformation.new(t2).to_a
      from_a(a.each_with_index.map { |v, i| v * (1 - w) + b[i] * w })
    end

    def to_a; @m.dup; end
    def clone; Transformation.from_a(@m); end
    alias dup clone
    def set!(t); @m = Transformation.new(t).to_a; self; end
    def identity?; @m.each_with_index.all? { |v, i| (v - IDENTITY_A[i]).abs < 1e-9 }; end
    def origin; Point3d.new(@m[12], @m[13], @m[14]); end
    def xaxis; Vector3d.new(@m[0], @m[1], @m[2]); end
    def yaxis; Vector3d.new(@m[4], @m[5], @m[6]); end
    def zaxis; Vector3d.new(@m[8], @m[9], @m[10]); end
    def xscale; xaxis.length; end
    def yscale; yaxis.length; end
    def zscale; zaxis.length; end

    def *(o)
      case o
      when Transformation
        a, b = @m, o.to_a
        c = Array.new(16, 0.0)
        4.times do |col|
          4.times do |row|
            c[col * 4 + row] = (0..3).inject(0.0) { |s, k| s + a[k * 4 + row] * b[col * 4 + k] }
          end
        end
        Transformation.from_a(c)
      when Vector3d
        v = o.to_a
        Vector3d.new(*(0..2).map { |r| @m[r] * v[0] + @m[4 + r] * v[1] + @m[8 + r] * v[2] })
      else
        p = SketchCAD.pt(o)
        w = @m[3] * p[0] + @m[7] * p[1] + @m[11] * p[2] + @m[15]
        Point3d.new(*(0..2).map { |r| (@m[r] * p[0] + @m[4 + r] * p[1] + @m[8 + r] * p[2] + @m[12 + r]) / w })
      end
    end

    def inverse
      # Gauss-Jordan on a row-major copy.
      a = (0..3).map { |r| (0..3).map { |c| @m[c * 4 + r] } }
      inv = (0..3).map { |r| (0..3).map { |c| r == c ? 1.0 : 0.0 } }
      4.times do |col|
        piv = (col..3).max_by { |r| a[r][col].abs }
        raise ArgumentError, 'Transformation is not invertible' if a[piv][col].abs < 1e-14
        a[col], a[piv] = a[piv], a[col]
        inv[col], inv[piv] = inv[piv], inv[col]
        d = a[col][col]
        4.times { |c| a[col][c] /= d; inv[col][c] /= d }
        4.times do |r|
          next if r == col
          f = a[r][col]
          next if f.zero?
          4.times { |c| a[r][c] -= f * a[col][c]; inv[r][c] -= f * inv[col][c] }
        end
      end
      Transformation.from_a((0..15).map { |i| inv[i % 4][i / 4] })
    end

    def invert!; set!(inverse); end
    def inspect; "Transformation(#{@m.map { |v| v.round(6) }.join(', ')})"; end
  end

  class BoundingBox
    attr_reader :min, :max

    def initialize
      clear
    end

    def clear
      @min = Point3d.new(Float::INFINITY, Float::INFINITY, Float::INFINITY)
      @max = Point3d.new(-Float::INFINITY, -Float::INFINITY, -Float::INFINITY)
      self
    end

    def add(*items)
      items.flatten.each do |i|
        if i.is_a?(BoundingBox)
          next if i.empty?
          add(i.min, i.max)
          next
        end
        p = SketchCAD.pt(i)
        @min = Point3d.new([@min.x, p[0]].min, [@min.y, p[1]].min, [@min.z, p[2]].min)
        @max = Point3d.new([@max.x, p[0]].max, [@max.y, p[1]].max, [@max.z, p[2]].max)
      end
      self
    end

    def empty?; @min.x > @max.x; end
    def valid?; !empty?; end
    def center; Point3d.linear_combination(0.5, @min, 0.5, @max); end
    def width; empty? ? 0.0 : @max.x - @min.x; end
    def height; empty? ? 0.0 : @max.y - @min.y; end
    def depth; empty? ? 0.0 : @max.z - @min.z; end
    def diagonal; empty? ? 0.0 : @min.distance(@max); end

    def corner(i)
      Point3d.new(i & 1 == 0 ? @min.x : @max.x, i & 2 == 0 ? @min.y : @max.y, i & 4 == 0 ? @min.z : @max.z)
    end

    def contains?(o)
      return contains?(o.min) && contains?(o.max) if o.is_a?(BoundingBox)
      p = SketchCAD.pt(o)
      (0..2).all? { |i| p[i] >= @min[i] - 0.001 && p[i] <= @max[i] + 0.001 }
    end

    def inspect; empty? ? 'BoundingBox(empty)' : "BoundingBox(#{@min.inspect}, #{@max.inspect})"; end
  end

  # Points (1-based) and polygons, for Entities#add_faces_from_mesh.
  class PolygonMesh
    def initialize(*_)
      @points = []
      @polygons = []
    end

    def add_point(p)
      p = Point3d.new(SketchCAD.pt(p))
      i = @points.index { |q| q == p }
      return i + 1 if i
      @points << p
      @points.length
    end

    def add_polygon(*a)
      a = a[0] if a.length == 1 && a[0].is_a?(Array) && !a[0][0].is_a?(Numeric)
      idx = a.map { |x| x.is_a?(Integer) ? x : add_point(x) }
      @polygons << idx
      @polygons.length
    end

    def point_at(i); @points[i.abs - 1]; end
    def points; @points.dup; end
    def polygons; @polygons.map(&:dup); end
    def polygon_at(i); @polygons[i - 1]; end
    def polygon_points_at(i); @polygons[i - 1].map { |x| point_at(x) }; end
    def count_points; @points.length; end
    def count_polygons; @polygons.length; end
    def point_index(p); (@points.index { |q| q == Point3d.new(SketchCAD.pt(p)) } || -1) + 1; end
  end

  def self.line_parts(line)
    p = Point3d.new(line[0])
    d = line[1].is_a?(Vector3d) ? line[1] : (Point3d.new(line[1]) - p)
    [p, d]
  end

  def self.plane_parts(plane)
    if plane.length == 4
      n = Vector3d.new(plane[0], plane[1], plane[2])
      l2 = n.dot(n)
      [Point3d.new(n.x * -plane[3] / l2, n.y * -plane[3] / l2, n.z * -plane[3] / l2), n.normalize]
    else
      [Point3d.new(plane[0]), SketchCAD.vec(plane[1]).normalize]
    end
  end

  def self.project_to_plane(pt, plane)
    o, n = plane_parts(plane)
    p = Point3d.new(pt)
    p - n.scale((p - o).dot(n))
  end

  def self.project_to_line(pt, line)
    o, d = line_parts(line)
    d = d.normalize
    o + d.scale((Point3d.new(pt) - o).dot(d))
  end

  def self.intersect_line_plane(line, plane)
    o, d = line_parts(line)
    po, n = plane_parts(plane)
    den = d.dot(n)
    return nil if den.abs < 1e-12
    o + d.scale((po - o).dot(n) / den)
  end

  def self.intersect_line_line(l1, l2)
    p1, d1 = line_parts(l1)
    p2, d2 = line_parts(l2)
    n = d1.cross(d2)
    return nil if n.length < 1e-12
    w = p1 - p2
    a, b, c = d1.dot(d1), d1.dot(d2), d2.dot(d2)
    d, e = d1.dot(w), d2.dot(w)
    den = a * c - b * b
    s = (b * e - c * d) / den
    t = (a * e - b * d) / den
    q1, q2 = p1 + d1.scale(s), p2 + d2.scale(t)
    q1.distance(q2) < 0.001 ? q1 : nil
  end

  def self.intersect_plane_plane(pl1, pl2)
    o1, n1 = plane_parts(pl1)
    o2, n2 = plane_parts(pl2)
    d = n1.cross(n2)
    return nil if d.length < 1e-12
    p = intersect_line_plane([o1, n1.cross(d)], pl2)
    p && [p, d.normalize]
  end

  def self.closest_points(l1, l2)
    p1, d1 = line_parts(l1)
    p2, d2 = line_parts(l2)
    w = p1 - p2
    a, b, c = d1.dot(d1), d1.dot(d2), d2.dot(d2)
    d, e = d1.dot(w), d2.dot(w)
    den = a * c - b * b
    return [p1, project_to_line(p1, l2)] if den.abs < 1e-12
    [p1 + d1.scale((b * e - c * d) / den), p2 + d2.scale((a * e - b * d) / den)]
  end

  def self.fit_plane_to_points(*pts)
    pts = pts.flatten(1).map { |p| SketchCAD.pt(p) }
    n = Vector3d.new
    c = [0.0, 0.0, 0.0]
    pts.each_with_index do |p, i|
      q = pts[(i + 1) % pts.length]
      n.x += (p[1] - q[1]) * (p[2] + q[2])
      n.y += (p[2] - q[2]) * (p[0] + q[0])
      n.z += (p[0] - q[0]) * (p[1] + q[1])
      3.times { |k| c[k] += p[k] / pts.length }
    end
    n = n.normalize
    [n.x, n.y, n.z, -(n.x * c[0] + n.y * c[1] + n.z * c[2])]
  end

  def self.linear_combination(w1, p1, w2, p2)
    Point3d.linear_combination(w1, p1, w2, p2)
  end

  def self.point_in_polygon_2D(pt, poly, border = false)
    x, y = SketchCAD.pt(pt)
    pts = poly.map { |p| SketchCAD.pt(p) }
    inside = false
    j = pts.length - 1
    pts.each_with_index do |pi, i|
      pj = pts[j]
      if border
        seg = Vector3d.new(pj[0] - pi[0], pj[1] - pi[1], 0)
        to = Vector3d.new(x - pi[0], y - pi[1], 0)
        len = seg.length
        if len > 0 && seg.cross(to).length / len < 0.001 && to.dot(seg).between?(0, len * len)
          return true
        end
      end
      if (pi[1] > y) != (pj[1] > y) && x < (pj[0] - pi[0]) * (y - pi[1]) / (pj[1] - pi[1]) + pi[0]
        inside = !inside
      end
      j = i
    end
    inside
  end
end

# SketchUp adds point/vector helpers to Array.
class Array
  def x; self[0]; end
  def y; self[1]; end
  def z; self[2]; end
  def x=(v); self[0] = v; end
  def y=(v); self[1] = v; end
  def z=(v); self[2] = v; end
  def distance(o); Geom::Point3d.new(self).distance(o); end
  def vector_to(o); Geom::Point3d.new(self).vector_to(o); end
  def offset(v, len = nil); Geom::Point3d.new(self).offset(v, len).to_a; end
  def transform(t); (Geom::Transformation.new(t) * Geom::Point3d.new(self)).to_a; end
  def cross(o); Geom::Vector3d.new(self).cross(o).to_a; end
  def dot(o); Geom::Vector3d.new(self).dot(o); end
  def normalize; Geom::Vector3d.new(self).normalize.to_a; end
  def project_to_plane(pl); Geom.project_to_plane(self, pl).to_a; end
  def project_to_line(l); Geom.project_to_line(self, l).to_a; end
  def distance_to_plane(pl); Geom::Point3d.new(self).distance_to_plane(pl); end
  def distance_to_line(l); Geom::Point3d.new(self).distance_to_line(l); end
  def on_plane?(pl); Geom::Point3d.new(self).on_plane?(pl); end
  def on_line?(l); Geom::Point3d.new(self).on_line?(l); end
end

ORIGIN = Geom::Point3d.new(0, 0, 0) unless defined?(ORIGIN)
X_AXIS = Geom::Vector3d.new(1, 0, 0) unless defined?(X_AXIS)
Y_AXIS = Geom::Vector3d.new(0, 1, 0) unless defined?(Y_AXIS)
Z_AXIS = Geom::Vector3d.new(0, 0, 1) unless defined?(Z_AXIS)
IDENTITY = Geom::Transformation.new unless defined?(IDENTITY)

# ---------------------------------------------------------------------------
# Sketchup

module Sketchup
  Host = SketchCAD::Host

  class Color
    NAMES = {
      'black' => [0, 0, 0], 'white' => [255, 255, 255], 'red' => [255, 0, 0], 'green' => [0, 128, 0],
      'lime' => [0, 255, 0], 'blue' => [0, 0, 255], 'yellow' => [255, 255, 0], 'cyan' => [0, 255, 255],
      'magenta' => [255, 0, 255], 'gray' => [128, 128, 128], 'grey' => [128, 128, 128],
      'silver' => [192, 192, 192], 'orange' => [255, 165, 0], 'purple' => [128, 0, 128],
      'brown' => [165, 42, 42], 'pink' => [255, 192, 203], 'navy' => [0, 0, 128], 'olive' => [128, 128, 0],
      'teal' => [0, 128, 128], 'maroon' => [128, 0, 0], 'gold' => [255, 215, 0], 'beige' => [245, 245, 220],
      'tan' => [210, 180, 140], 'khaki' => [240, 230, 140], 'salmon' => [250, 128, 114],
      'darkgreen' => [0, 100, 0], 'darkblue' => [0, 0, 139], 'darkred' => [139, 0, 0],
      'darkgray' => [169, 169, 169], 'lightgray' => [211, 211, 211], 'lightblue' => [173, 216, 230],
      'forestgreen' => [34, 139, 34], 'skyblue' => [135, 206, 235], 'chocolate' => [210, 105, 30],
      'sienna' => [160, 82, 45], 'wheat' => [245, 222, 179], 'ivory' => [255, 255, 240]
    }.freeze

    attr_accessor :red, :green, :blue, :alpha

    def self.names; NAMES.keys; end

    def initialize(*a)
      a = a[0] if a.length == 1 && a[0].is_a?(Array)
      if a.empty?
        @red, @green, @blue, @alpha = 0, 0, 0, 255
      elsif a.length == 1 && a[0].is_a?(Color)
        @red, @green, @blue, @alpha = a[0].to_a
      elsif a.length == 1 && a[0].is_a?(String)
        s = a[0].strip
        if (m = s.match(/\A#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})\z/i))
          @red, @green, @blue = m[1].hex, m[2].hex, m[3].hex
        else
          rgb = NAMES[s.downcase.delete(' ')] || raise(ArgumentError, "unknown color '#{s}'")
          @red, @green, @blue = rgb
        end
        @alpha = 255
      elsif a.length == 1 && a[0].is_a?(Integer)
        v = a[0]
        @red, @green, @blue, @alpha = v & 0xff, (v >> 8) & 0xff, (v >> 16) & 0xff, 255
      else
        vals = a.map(&:to_f)
        vals = vals.map { |v| v * 255.0 } if vals.all? { |v| v <= 1.0 } && a.any? { |v| v.is_a?(Float) }
        @red, @green, @blue = vals[0].round, vals[1].round, vals[2].round
        @alpha = (vals[3] || 255).round
      end
    end

    def to_a; [@red, @green, @blue, @alpha]; end
    def to_i; @red | (@green << 8) | (@blue << 16); end
    def ==(o); o.is_a?(Color) && o.to_a == to_a; end

    def blend(o, w)
      o = Color.new(o)
      Color.new(*to_a.each_with_index.map { |v, i| (v * w + o.to_a[i] * (1 - w)).round })
    end

    def inspect; "Color(#{@red}, #{@green}, #{@blue}, #{@alpha})"; end
    alias to_s inspect
  end

  class Material
    attr_reader :name
    attr_accessor :color, :texture, :materialType

    def initialize(name, color = Color.new(255, 255, 255))
      @name = name
      @color = color
      @texture = nil
      @materialType = 0
    end

    def name=(n); @name = n.to_s; end
    def display_name; @name; end
    def color=(c); @color = Color.new(c); end
    def alpha; @color.alpha / 255.0; end
    def alpha=(a); @color.alpha = (a.to_f * 255).round; end
    def use_alpha?; @color.alpha < 255; end
    def texture=(_t); Host.warn_once(:texture, 'textures are not supported yet; using the color only'); end
    def valid?; true; end
    def inspect; "#<Sketchup::Material #{@name}>"; end
  end

  class Materials
    include Enumerable

    def initialize
      @list = []
      @current = nil
    end

    attr_accessor :current

    def add(name)
      m = Material.new(unique_name(name.to_s))
      @list << m
      m
    end

    def [](k)
      return @list[k] if k.is_a?(Integer)
      @list.find { |m| m.name == k.to_s }
    end

    def for_color(c)
      @list.find { |m| m.color.to_a == c.to_a } || begin
        m = Material.new(format('Color_%02X%02X%02X', c.red, c.green, c.blue), c)
        @list << m
        m
      end
    end

    def unique_name(base)
      return base unless self[base]
      (1..).each { |i| n = "#{base}#{i}"; return n unless self[n] }
    end

    def remove(m); !@list.delete(m.is_a?(Material) ? m : self[m]).nil?; end
    def each(&b); @list.dup.each(&b); end
    def length; @list.length; end
    alias size length
    alias count length
    def purge_unused; 0; end
    def add_observer(_o); Host.warn_once(:observers, 'observers are not supported yet; they will not be called'); true; end
    def remove_observer(_o); true; end
  end

  class Layer
    attr_reader :name

    def initialize(name, visible = true)
      @name = name
      @visible = visible
    end

    def visible?; @visible; end

    def visible=(v)
      Host.request('tags.set_visible', @name, v ? true : false)
      @visible = v
    end

    def ==(o); o.is_a?(Layer) && o.name == @name; end
    def display_name; @name; end
    def valid?; true; end
    def inspect; "#<Sketchup::Layer #{@name}>"; end
  end

  LAYER0 = Layer.new('Layer0')

  class Layers
    include Enumerable

    def each(&b)
      ([LAYER0] + Host.request('tags.list').map { |t| Layer.new(t['name'], t['visible']) }).each(&b)
    end

    def add(name)
      Host.request('tags.add', name.to_s)
      Layer.new(name.to_s)
    end

    def [](k)
      return to_a[k] if k.is_a?(Integer)
      find { |l| l.name == k.to_s }
    end

    def length; to_a.length; end
    alias size length
    alias count length
    def unique_name(base = 'Layer'); (1..).each { |i| n = "#{base}#{i}"; return n unless self[n] }; end
    def add_observer(_o); Host.warn_once(:observers, 'observers are not supported yet; they will not be called'); true; end
    def remove_observer(_o); true; end
  end

  class AttributeDictionary
    include Enumerable
    attr_reader :name

    def initialize(name, hash)
      @name = name
      @h = hash
    end

    def [](k); @h[k.to_s]; end
    def []=(k, v); @h[k.to_s] = v; end
    def keys; @h.keys; end
    def values; @h.values; end
    def each(&b); @h.each(&b); end
    alias each_pair each
    def delete_key(k); @h.delete(k.to_s); end
    def length; @h.length; end
    alias size length
  end

  # Base of everything in the model.
  class Entity
    attr_reader :_handle

    def initialize(h)
      @_handle = h
    end

    def ==(o); o.is_a?(Entity) && o._handle == @_handle; end
    alias eql? ==
    def hash; @_handle.hash; end
    def entityID; @_handle.split(':').last.to_i % 2_147_483_647; end
    alias persistent_id entityID
    def typename; self.class.name.split('::').last; end
    def valid?; Host.request('entity.valid', @_handle); end
    def deleted?; !valid?; end
    def model; Sketchup.active_model; end
    def _ctx; @_handle.split(':')[1]; end

    def parent
      c = _ctx
      c == '0' ? Sketchup.active_model : Host.wrap("D:#{c}")
    end

    def _attrs; SketchCAD::ATTRIBUTES[@_handle]; end

    def get_attribute(dict, key, default = nil)
      d = _attrs[dict.to_s]
      d && d.key?(key.to_s) ? d[key.to_s] : default
    end

    def set_attribute(dict, key, value)
      Host.warn_once(:attrs, 'attribute dictionaries are kept for this session only (not saved in .skcad files yet)')
      (_attrs[dict.to_s] ||= {})[key.to_s] = value
    end

    def delete_attribute(dict, key = nil)
      return !_attrs.delete(dict.to_s).nil? if key.nil?
      d = _attrs[dict.to_s]
      d ? !d.delete(key.to_s).nil? : false
    end

    def attribute_dictionary(name, create = false)
      return nil unless _attrs.key?(name.to_s) || create
      AttributeDictionary.new(name.to_s, _attrs[name.to_s] ||= {})
    end

    def attribute_dictionaries
      _attrs.map { |k, v| AttributeDictionary.new(k, v) }
    end

    def add_observer(_o); Host.warn_once(:observers, 'observers are not supported yet; they will not be called'); true; end
    def remove_observer(_o); true; end
    def inspect; "#<#{self.class.name}:#{@_handle}>"; end
    alias to_s inspect
  end

  class Drawingelement < Entity
    def bounds
      r = Host.request('entity.bounds', @_handle)
      bb = Geom::BoundingBox.new
      bb.add(r[0], r[1]) if r
      bb
    end

    def erase!
      Host.request('ents.erase', [@_handle])
      true
    end

    def layer; LAYER0; end
    def layer=(_l); Host.warn_once(:layer, 'tags can only be assigned to groups and components'); end
    def casts_shadows?; true; end
    def receives_shadows?; true; end
    def hidden?; false; end
    def visible?; !hidden?; end
    def visible=(v); self.hidden = !v; end
    def hidden=(_v); Host.warn_once(:hide, "#{typename} cannot be hidden in SketchCAD"); end
  end

  class Vertex < Entity
    def _info; Host.request('vertex.info', @_handle); end
    def position; Geom::Point3d.new(_info['position']); end
    def edges; _info['edges'].map { |h| Host.wrap(h) }; end
    def faces; _info['faces'].map { |h| Host.wrap(h) }; end
    def used_by?(e); e.respond_to?(:vertices) && e.vertices.include?(self); end
    def common_edge(v); edges.find { |e| e.other_vertex(self) == v }; end
    def curve_interior?; false; end
  end

  class Curve < Entity
    def initialize(edges)
      @edges = edges
      super(edges.first ? edges.first._handle : '')
    end

    def edges; @edges; end
    def first_edge; @edges.first; end
    def last_edge; @edges.last; end
    def count_edges; @edges.length; end
    def length; @edges.inject(0.0) { |s, e| s + e.length }; end
    def vertices; @edges.flat_map(&:vertices).uniq; end
    def each_edge(&b); @edges.each(&b); end
  end

  class ArcCurve < Curve; end

  class Edge < Drawingelement
    def _info; Host.request('edge.info', @_handle); end
    def start; Host.wrap(_info['start'][0]); end
    def end; Host.wrap(_info['end'][0]); end

    def vertices
      i = _info
      [Host.wrap(i['start'][0]), Host.wrap(i['end'][0])]
    end

    def length; _info['length']; end
    def faces; _info['faces'].map { |h| Host.wrap(h) }; end

    def line
      i = _info
      a, b = Geom::Point3d.new(i['start'][1]), Geom::Point3d.new(i['end'][1])
      [a, (b - a).normalize]
    end

    def other_vertex(v)
      a, b = vertices
      v == a ? b : (v == b ? a : nil)
    end

    def used_by?(o); o.is_a?(Vertex) ? vertices.include?(o) : faces.include?(o); end
    def common_face(e); (faces & e.faces).first; end
    def reversed_in?(_face); false; end
    def soft?; _info['soft']; end
    def smooth?; _info['smooth']; end
    def hidden?; _info['hidden']; end
    def soft=(v); Host.request('edge.set', @_handle, 'soft' => v ? true : false); end
    def smooth=(v); Host.request('edge.set', @_handle, 'smooth' => v ? true : false); end
    def hidden=(v); Host.request('edge.set', @_handle, 'hidden' => v ? true : false); end
    def find_faces; 0; end

    def curve
      c = _info['curve']
      c.empty? ? nil : Curve.new(c.map { |h| Host.wrap(h) })
    end

    def explode_curve; self; end
    def material; nil; end
    def material=(_m); Host.warn_once(:edge_mat, 'edge materials are not supported yet'); end
  end

  class Loop < Entity
    def initialize(face, verts, outer)
      super(face._handle)
      @face, @verts, @outer = face, verts, outer
    end

    def face; @face; end
    def outer?; @outer; end
    def vertices; @verts.map { |v| Host.wrap(v[0]) }; end

    def edges
      vs = vertices
      vs.each_with_index.map { |v, i| v.common_edge(vs[(i + 1) % vs.length]) }.compact
    end

    def convex?
      pts = @verts.map { |v| Geom::Point3d.new(v[1]) }
      n = @face.normal
      pts.each_with_index.all? do |p, i|
        a = pts[(i + 1) % pts.length] - p
        b = pts[(i + 2) % pts.length] - pts[(i + 1) % pts.length]
        a.cross(b).dot(n) >= -1e-9
      end
    end
  end

  class Face < Drawingelement
    def _info; Host.request('face.info', @_handle); end
    def normal; Geom::Vector3d.new(_info['normal']); end
    def plane; _info['plane']; end
    def area; _info['area']; end
    def edges; _info['edges'].map { |h| Host.wrap(h) }; end

    def vertices
      i = _info
      (i['outer'] + i['holes'].flatten(1)).map { |v| Host.wrap(v[0]) }
    end

    def outer_loop; Loop.new(self, _info['outer'], true); end

    def loops
      i = _info
      [Loop.new(self, i['outer'], true)] + i['holes'].map { |h| Loop.new(self, h, false) }
    end

    def pushpull(distance, copy = false)
      Host.request('face.pushpull', @_handle, distance.to_f, copy ? true : false)
      nil
    end

    def reverse!
      Host.request('face.reverse', @_handle)
      self
    end

    def material; SketchCAD.material_for(_info['front']); end
    def back_material; SketchCAD.material_for(_info['back']); end
    def material=(m); Host.request('face.set_material', @_handle, SketchCAD.color_value(m), false); end
    def back_material=(m); Host.request('face.set_material', @_handle, SketchCAD.color_value(m), true); end

    def followme(*path)
      edges = path.flatten.flat_map { |e| e.is_a?(Curve) ? e.edges : [e] }
      Host.request('face.followme', @_handle, SketchCAD.handles(edges))
    end

    def classify_point(pt)
      p = Geom::Point3d.new(pt)
      return 32 unless p.on_plane?(plane) # PointNotOnPlane
      poly = outer_loop.vertices.map(&:position)
      Geom.point_in_polygon_2D(_to_2d(p), poly.map { |q| _to_2d(q) }, false) ? 1 : 16 # PointInside / PointOutside
    end

    def _to_2d(p)
      x, y, = normal.axes
      [p.to_a.dot(x.to_a), p.to_a.dot(y.to_a), 0]
    end

    def all_connected; [self] + edges; end
    def mesh(*_); raise NotImplementedError, 'Face#mesh is not supported by SketchCAD yet'; end
  end

  module InstanceMethods
    def _info; Host.request('inst.info', @_handle); end
    def definition; Host.wrap(_info['definition']); end
    def transformation; Geom::Transformation.new(_info['transformation']); end

    def transformation=(t)
      Host.request('inst.set', @_handle, 'transformation' => Geom::Transformation.new(t).to_a)
    end

    def move!(t); self.transformation = t; true; end
    def transform!(t); self.transformation = Geom::Transformation.new(t) * transformation; true; end
    def name; _info['name']; end
    def name=(n); Host.request('inst.set', @_handle, 'name' => n.to_s); end
    def locked?; _info['locked']; end
    def locked=(v); Host.request('inst.set', @_handle, 'locked' => v ? true : false); end
    def hidden?; _info['hidden']; end
    def hidden=(v); Host.request('inst.set', @_handle, 'hidden' => v ? true : false); end

    def layer
      l = _info['layer']
      l ? Layer.new(l) : LAYER0
    end

    def layer=(l)
      n = l.is_a?(Layer) ? l.name : l.to_s
      Host.request('inst.set', @_handle, 'layer' => (n == 'Layer0' || n == 'Untagged' ? nil : n))
    end

    def explode; Host.request('inst.explode', @_handle).map { |h| Host.wrap(h) }; end
    def make_unique; Host.wrap(Host.request('inst.make_unique', @_handle)); end
    def material; nil; end
    def material=(_m); Host.warn_once(:inst_mat, 'materials on groups/components are not supported yet; paint the faces inside'); end
    def guid; @_handle; end
    def glued_to; nil; end

    def copy
      ents = parent.is_a?(Sketchup::Model) ? parent.entities : parent.entities
      c = ents.add_instance(definition, transformation)
      c = c.make_unique if is_a?(Group)
      c
    end
  end

  class ComponentInstance < Drawingelement
    include InstanceMethods
  end

  class Group < Drawingelement
    include InstanceMethods
    def entities; Entities.new(_info['entities']); end
    def description; definition.description; end
    def description=(d); definition.description = d; end
    def manifold?; false; end
  end

  class ComponentDefinition < Drawingelement
    def _info; Host.request('def.info', @_handle); end
    def name; _info['name']; end
    def name=(n); Host.request('def.set', @_handle, 'name' => n.to_s); end
    def description; _info['description']; end
    def description=(d); Host.request('def.set', @_handle, 'description' => d.to_s); end
    def entities; Entities.new(_info['entities']); end
    def instances; _info['instances'].map { |h| Host.wrap(h) }; end
    def count_instances; instances.length; end
    alias count_used_instances count_instances
    def group?; _info['group']; end
    def image?; false; end
    def internal?; false; end
    def path; ''; end
    def insertion_point; Geom::Point3d.new; end
    def invalidate_bounds; nil; end
    def behavior; @behavior ||= Behavior.new; end
  end

  class Behavior
    attr_accessor :always_face_camera, :cuts_opening, :is2d, :no_scale_mask, :shadows_face_sun, :snapto
    def always_face_camera?; @always_face_camera ? true : false; end
    def cuts_opening?; @cuts_opening ? true : false; end
    def is2d?; @is2d ? true : false; end
    def shadows_face_sun?; false; end
  end

  class DefinitionList
    include Enumerable

    def each(&b); Host.request('defs.list').map { |h| Host.wrap(h) }.each(&b); end

    def [](k)
      return to_a[k] if k.is_a?(Integer)
      find { |d| d.name == k.to_s }
    end

    def add(name); Host.wrap(Host.request('defs.add', name.to_s)); end
    def length; to_a.length; end
    alias size length
    alias count length
    def unique_name(base); (1..).each { |i| n = "#{base}##{i}"; return n unless self[n] }; end
    def purge_unused; 0; end
    def load(_path, *_); raise NotImplementedError, 'loading .skp components is not supported by SketchCAD'; end
    def add_observer(_o); Host.warn_once(:observers, 'observers are not supported yet; they will not be called'); true; end
    def remove_observer(_o); true; end
  end

  class EntitiesBuilder
    def initialize(ents); @ents = ents; @valid = true; end
    def valid?; @valid; end
    def add_face(*a); @ents.add_face(*a); end
    def add_edge(*a); @ents.add_line(*a); end
    def add_edges(*a); @ents.add_edges(*a); end
    def add_line(*a); @ents.add_line(*a); end
    def entities; @ents; end
    def _finish; @valid = false; end
  end

  class Entities
    include Enumerable
    attr_reader :_ctx

    def initialize(ctx)
      @_ctx = ctx
    end

    def to_a; Host.request('ents.list', @_ctx).map { |h| Host.wrap(h) }; end
    def each(&b); to_a.each(&b); end
    def [](i); to_a[i]; end
    def length; to_a.length; end
    alias size length

    def count(*a, &b)
      return length if a.empty? && b.nil?
      super
    end

    def model; Sketchup.active_model; end
    def parent; @_ctx == '0' ? Sketchup.active_model : Host.wrap("D:#{@_ctx}"); end
    def ==(o); o.is_a?(Entities) && o._ctx == @_ctx; end

    def add_line(*a)
      pts = SketchCAD.point_list(a)
      raise ArgumentError, 'add_line needs two points' unless pts.length == 2
      Host.wrap(Host.request('ents.add_line', @_ctx, pts[0], pts[1]))
    end

    def add_edges(*a)
      Host.request('ents.add_edges', @_ctx, SketchCAD.point_list(a), false).map { |h| Host.wrap(h) }
    end

    def add_curve(*a)
      Host.request('ents.add_curve', @_ctx, SketchCAD.point_list(a), false).map { |h| Host.wrap(h) }
    end

    def add_face(*a)
      list = a.flatten(1)
      pts = if !list.empty? && list.all? { |x| x.is_a?(Edge) || x.is_a?(Curve) }
              SketchCAD.loop_points(list)
            else
              SketchCAD.point_list(a)
            end
      Host.wrap(Host.request('ents.add_face', @_ctx, pts))
    end

    def add_circle(center, normal, radius, numsegs = 24)
      c = Geom::Point3d.new(center)
      xa, ya, = SketchCAD.vec(normal).axes
      pts = (0...numsegs).map do |i|
        t = 2 * Math::PI * i / numsegs
        c + xa.scale(radius * Math.cos(t)) + ya.scale(radius * Math.sin(t))
      end
      Host.request('ents.add_curve', @_ctx, pts.map(&:to_a), true).map { |h| Host.wrap(h) }
    end

    def add_ngon(center, normal, radius, numsides = 24)
      c = Geom::Point3d.new(center)
      xa, ya, = SketchCAD.vec(normal).axes
      pts = (0...numsides).map do |i|
        t = 2 * Math::PI * i / numsides
        c + xa.scale(radius * Math.cos(t)) + ya.scale(radius * Math.sin(t))
      end
      Host.request('ents.add_curve', @_ctx, pts.map(&:to_a), true).map { |h| Host.wrap(h) }
    end

    def add_arc(center, xaxis, normal, radius, start_angle, end_angle, numsegs = 12)
      c = Geom::Point3d.new(center)
      xa = SketchCAD.vec(xaxis).normalize
      ya = SketchCAD.vec(normal).normalize.cross(xa).normalize
      pts = (0..numsegs).map do |i|
        t = start_angle + (end_angle - start_angle) * i / numsegs
        c + xa.scale(radius * Math.cos(t)) + ya.scale(radius * Math.sin(t))
      end
      Host.request('ents.add_curve', @_ctx, pts.map(&:to_a), false).map { |h| Host.wrap(h) }
    end

    def add_group(*ents)
      Host.wrap(Host.request('ents.add_group', @_ctx, SketchCAD.handles(ents)))
    end

    def add_instance(definition, transform)
      t = Geom::Transformation.new(transform)
      Host.wrap(Host.request('ents.add_instance', @_ctx, definition._handle, t.to_a))
    end

    def add_cpoint(pt)
      Host.request('ents.add_guide', @_ctx, SketchCAD.pt(pt), nil)
      nil
    end

    def add_cline(start, other, _stipple = nil)
      s = Geom::Point3d.new(start)
      dir = other.is_a?(Geom::Vector3d) ? other : (Geom::Point3d.new(other) - s)
      Host.request('ents.add_guide', @_ctx, s.to_a, dir.to_a)
      nil
    end

    def add_faces_from_mesh(mesh, _smooth = 0, material = nil, back_material = nil)
      n = 0
      mesh.count_polygons.times do |i|
        f = add_face(mesh.polygon_points_at(i + 1))
        next unless f
        f.material = material if material
        f.back_material = back_material if back_material
        n += 1
      end
      n
    end

    def fill_from_mesh(mesh, _weld = true, _flags = 0, material = nil, back_material = nil)
      add_faces_from_mesh(mesh, 0, material, back_material) > 0
    end

    def build
      b = EntitiesBuilder.new(self)
      yield b
      b._finish
      nil
    end

    def erase_entities(*ents)
      Host.request('ents.erase', SketchCAD.handles(ents))
      nil
    end

    def transform_entities(t, *ents)
      Host.request('ents.transform', @_ctx, Geom::Transformation.new(t).to_a, SketchCAD.handles(ents))
      true
    end

    def clear!
      Host.request('ents.clear', @_ctx)
      true
    end

    def active_section_plane; nil; end
    def add_text(*_); raise NotImplementedError, 'text is not supported by SketchCAD yet'; end
    def add_3d_text(*_); raise NotImplementedError, '3D text is not supported by SketchCAD yet'; end
    def add_dimension_linear(*_); raise NotImplementedError, 'dimensions are not supported by SketchCAD yet'; end
    def add_section_plane(*_); raise NotImplementedError, 'section planes are not supported by SketchCAD yet'; end
    def intersect_with(*_); raise NotImplementedError, 'Intersect Faces is not supported by SketchCAD yet'; end
    def weld(edges); edges; end
    def add_observer(_o); Host.warn_once(:observers, 'observers are not supported yet; they will not be called'); true; end
    def remove_observer(_o); true; end
    def inspect; "#<Sketchup::Entities #{@_ctx}>"; end
  end

  class Selection
    include Enumerable

    def to_a; Host.request('sel.get').map { |h| Host.wrap(h) }; end
    def each(&b); to_a.each(&b); end
    def length; to_a.length; end
    alias size length
    alias nitems length

    def count(*a, &b)
      return length if a.empty? && b.nil?
      super
    end

    def empty?; length.zero?; end
    alias is_empty? empty?
    def first; to_a.first; end
    def [](i); to_a[i]; end
    alias at []
    def single_object?; length == 1; end
    def contains?(e); to_a.include?(e); end
    alias include? contains?
    alias is_selected? contains?
    def model; Sketchup.active_model; end
    def _set(list); Host.request('sel.set', SketchCAD.handles(list)); list.length; end
    def add(*e); _set((to_a + e.flatten).uniq); end
    def remove(*e); _set(to_a - e.flatten); end
    alias delete remove
    def toggle(*e); cur = to_a; e = e.flatten; _set((cur - e) + (e - cur)); end
    def clear; _set([]); nil; end
    def invert; _set(model.active_entities.to_a - to_a); end
    def shift; f = first; remove(f) if f; f; end
    def add_observer(_o); Host.warn_once(:observers, 'observers are not supported yet; they will not be called'); true; end
    def remove_observer(_o); true; end
  end

  class Camera
    attr_reader :eye, :target, :up, :fov

    def initialize(eye = [0, -1000, 500], target = [0, 0, 0], up = [0, 0, 1], perspective = true, fov = 35.0)
      @eye = Geom::Point3d.new(eye)
      @target = Geom::Point3d.new(target)
      @up = Geom::Vector3d.new(up)
      @perspective = perspective
      @fov = fov.to_f
      @live = false
    end

    def _live!; @live = true; self; end
    def _push; Host.request('camera.set', _to_h) if @live; end
    def _to_h; { 'eye' => @eye.to_a, 'target' => @target.to_a, 'fov' => @fov, 'perspective' => @perspective }; end
    def direction; (@target - @eye).normalize; end
    def xaxis; direction.cross(@up).normalize; end
    def yaxis; xaxis.cross(direction).normalize; end
    def zaxis; direction; end
    def perspective?; @perspective; end
    def perspective=(v); @perspective = v ? true : false; _push; end
    def fov=(v); @fov = v.to_f; _push; end
    def height; 2 * @eye.distance(@target) * Math.tan(@fov.degrees / 2); end

    def set(eye, target, up)
      @eye, @target, @up = Geom::Point3d.new(eye), Geom::Point3d.new(target), Geom::Vector3d.new(up)
      _push
      self
    end

    def aspect_ratio; 0.0; end
  end

  class View
    def model; Sketchup.active_model; end

    def camera
      c = Host.request('camera.get')
      Camera.new(c['eye'], c['target'], c['up'], c['perspective'], c['fov'])._live!
    end

    def camera=(c)
      c = c[0] if c.is_a?(Array)
      Host.request('camera.set', c._to_h)
    end

    def zoom_extents; Host.request('view.zoom_extents'); self; end
    def zoom(_x = nil); zoom_extents; end
    def refresh; self; end
    def invalidate; self; end
    def vpwidth; Host.request('view.size')[0]; end
    def vpheight; Host.request('view.size')[1]; end
    def show_frame(*_); true; end
    def animation=(_a); Host.warn_once(:anim, 'view animations are not supported yet'); end
    def add_observer(_o); Host.warn_once(:observers, 'observers are not supported yet; they will not be called'); true; end
    def remove_observer(_o); true; end
  end

  class OptionsProvider
    def initialize(h); @h = h; end
    def [](k); @h[k.to_s]; end
    def []=(k, v); @h[k.to_s] = v; end
    def keys; @h.keys; end
    def each(&b); @h.each(&b); end
    def has_key?(k); @h.key?(k.to_s); end
  end

  class OptionsManager
    UNITS = { 'Inches' => 0, 'Feet' => 1, 'Millimeters' => 2, 'Centimeters' => 3, 'Meters' => 4, 'Architectural' => 0 }.freeze

    def [](k)
      return nil unless k.to_s == 'UnitsOptions'
      unit = Sketchup.active_model._info['unit']
      OptionsProvider.new('LengthUnit' => UNITS[unit] || 2, 'LengthFormat' => unit == 'Architectural' ? 1 : 0,
                          'LengthPrecision' => 2, 'AngleSnapEnabled' => true, 'SnapAngle' => 15.0)
    end

    def keys; ['UnitsOptions']; end
    def each; yield self['UnitsOptions']; end
  end

  class Tools
    def push_tool(_t); raise NotImplementedError, 'custom Ruby tools are not supported by SketchCAD yet'; end
    def pop_tool; nil; end
    def active_tool_name; 'SelectionTool'; end
    def active_tool_id; 21_022; end
    def add_observer(_o); Host.warn_once(:observers, 'observers are not supported yet; they will not be called'); true; end
    def remove_observer(_o); true; end
  end

  class Pages
    include Enumerable
    def each(&_b); []; end
    def length; 0; end
    alias size length
    alias count length
    def [](_k); nil; end
    def selected_page; nil; end
    def add(*_); raise NotImplementedError, 'creating scenes from Ruby is not supported by SketchCAD yet'; end
  end

  class Model
    def _info; Host.request('model.info'); end
    def entities; Entities.new('0'); end
    def active_entities; Entities.new(_info['active']); end
    def selection; @selection ||= Selection.new; end
    def definitions; @definitions ||= DefinitionList.new; end
    def materials; @materials ||= Materials.new; end
    def layers; @layers ||= Layers.new; end
    alias tags layers
    def active_view; @view ||= View.new; end
    def options; @options ||= OptionsManager.new; end
    def tools; @tools ||= Tools.new; end
    def pages; @pages ||= Pages.new; end
    def shadow_info; @shadow_info ||= OptionsProvider.new('DisplayShadows' => false); end
    def rendering_options; @rendering_options ||= OptionsProvider.new({}); end
    def active_layer; LAYER0; end
    def active_layer=(_l); nil; end

    def start_operation(name = '', _disable_ui = false, _next_transparent = false, _transparent = false)
      Host.request('op.start', name.to_s)
    end

    def commit_operation; Host.request('op.commit'); end
    def abort_operation; Host.request('op.abort'); end
    def title; _info['title']; end
    def name; title; end
    def path; _info['path']; end
    def modified?; _info['modified']; end
    def valid?; true; end
    def guid; 'sketchcad-model'; end
    def description; ''; end

    def save(path = nil)
      Host.request('model.save', path.to_s)
    end

    def bounds
      r = Host.request('model.bounds')
      bb = Geom::BoundingBox.new
      bb.add(r[0], r[1]) if r
      bb
    end

    def active_path
      p = Host.request('model.active_path').map { |h| Host.wrap(h) }
      p.empty? ? nil : p
    end

    def close_active; Host.request('model.close_active'); end
    def edit_transform; Geom::Transformation.new; end

    def select_tool(tool)
      return Sketchup.send_action('selectSelectionTool:') if tool.nil?
      raise NotImplementedError, 'custom Ruby tools are not supported by SketchCAD yet'
    end

    def place_component(*_); raise NotImplementedError, 'place_component is not supported by SketchCAD yet'; end
    def import(*_); raise NotImplementedError, 'import is not supported by SketchCAD yet'; end
    def export(*_); raise NotImplementedError, 'export is not supported by SketchCAD yet'; end
    def number_faces; entities.grep(Face).length; end
    def get_attribute(d, k, default = nil); (SketchCAD::ATTRIBUTES['model'][d.to_s] || {}).fetch(k.to_s, default); end
    def set_attribute(d, k, v); (SketchCAD::ATTRIBUTES['model'][d.to_s] ||= {})[k.to_s] = v; end
    def attribute_dictionary(name, create = false)
      a = SketchCAD::ATTRIBUTES['model']
      return nil unless a.key?(name.to_s) || create
      AttributeDictionary.new(name.to_s, a[name.to_s] ||= {})
    end
    def add_observer(_o); Host.warn_once(:observers, 'observers are not supported yet; they will not be called'); true; end
    def remove_observer(_o); true; end
    def inspect; '#<Sketchup::Model>'; end
  end

  # Observer base classes, so extensions that subclass them still load.
  %w[AppObserver ModelObserver EntitiesObserver EntityObserver SelectionObserver ToolsObserver ViewObserver
     DefinitionObserver DefinitionsObserver LayersObserver MaterialsObserver FrameChangeObserver
     InstanceObserver PagesObserver RenderingOptionsObserver ShadowInfoObserver OptionsProviderObserver].each do |n|
    const_set(n, Class.new) unless const_defined?(n, false)
  end

  class ExtensionsManager
    include Enumerable

    def initialize; @list = {}; end
    def _add(ext); @list[ext.name] = ext; end
    def each(&b); @list.values.each(&b); end
    def [](k); k.is_a?(Integer) ? @list.values[k] : @list[k.to_s]; end
    def length; @list.length; end
    alias size length
    alias count length
    def keys; @list.keys; end
  end

  def self.active_model; @model ||= Model.new; end
  def self.version; '24.0.0'; end
  def self.version_number; 24_000_000; end
  def self.platform; RUBY_PLATFORM =~ /mswin|mingw/ ? :platform_win : :platform_osx; end
  def self.is_pro?; true; end
  def self.is_online; false; end
  def self.app_name; 'SketchCAD'; end
  def self.os_language; 'en-US'; end
  def self.get_locale; 'en-US'; end
  def self.temp_dir; Dir.tmpdir; end
  def self.debug_mode?; false; end
  def self.break_edges?; true; end
  def self.extensions; @extensions ||= ExtensionsManager.new; end
  def self.require(path); SketchCAD.require_file(path); end
  def self.load(path); SketchCAD.require_file(path); end
  def self.plugins_disabled?; false; end

  def self.find_support_file(name, dir = '')
    p = File.join(Host.plugins_dir.to_s, dir.to_s, name.to_s)
    File.exist?(p) ? p : nil
  end

  def self.find_support_files(ext, dir = '')
    Dir.glob(File.join(Host.plugins_dir.to_s, dir.to_s, "*.#{ext}")).sort
  end

  def self.register_extension(ext, load_on_start = false)
    loader = Host.current_loader || caller_locations.map(&:absolute_path).compact.find { |p| !p.start_with?(SketchCAD::API_DIR) }
    ext._register(load_on_start, loader)
    true
  end

  def self.status_text=(t); Host.note('status', 'text' => t.to_s); end
  def self.set_status_text(t = '', _pos = nil); self.status_text = t; end
  def self.send_action(name); Host.note('action', 'name' => name.to_s); true; end
  def self.undo; send_action('editUndo:'); end
  def self.file_new; send_action('newDocument:'); end
  def self.open_file(*_); raise NotImplementedError, 'Sketchup.open_file is not supported by SketchCAD yet'; end
  def self.quit; nil; end
  def self.add_observer(_o); Host.warn_once(:observers, 'observers are not supported yet; they will not be called'); true; end
  def self.remove_observer(_o); true; end
  def self.display_name; 'SketchCAD'; end
  def self.fix_shadow_strings?; false; end
  def self.focus; true; end

  def self.read_default(section, key, default = nil)
    (SketchCAD.prefs[section.to_s] || {}).fetch(key.to_s, default)
  end

  def self.write_default(section, key, value)
    p = SketchCAD.prefs
    (p[section.to_s] ||= {})[key.to_s] = value
    File.write(SketchCAD.prefs_path, JSON.generate(p))
    true
  rescue StandardError
    false
  end
end

# ---------------------------------------------------------------------------
# UI

module UI
  Host = SketchCAD::Host

  MENUS = {
    'plugins' => 'Extensions', 'extensions' => 'Extensions', 'file' => 'File', 'edit' => 'Edit', 'view' => 'View',
    'camera' => 'Camera', 'draw' => 'Draw', 'tools' => 'Tools', 'window' => 'Window', 'help' => 'Help'
  }.freeze

  def self.menu(name = 'Extensions')
    Menu.new(MENUS[name.to_s.downcase] || 'Extensions', [])
  end

  class Menu
    def initialize(menu, path)
      @menu = menu
      @path = path
    end

    def add_item(arg, _index = nil, &blk)
      label = arg
      if arg.is_a?(Command)
        label = arg.menu_text
        blk = arg.proc
      end
      raise ArgumentError, 'add_item needs a block or a UI::Command' unless blk
      id = Host.register_proc(blk)
      Host.note('menu.item', 'menu' => @menu, 'path' => @path, 'label' => label.to_s, 'proc' => id)
      id
    end

    def add_submenu(label, _index = nil); Menu.new(@menu, @path + [label.to_s]); end

    def add_separator
      Host.note('menu.separator', 'menu' => @menu, 'path' => @path)
      nil
    end

    def set_validation_proc(_id, &_b); true; end
  end

  class Command
    attr_accessor :menu_text, :tooltip, :status_bar_text, :small_icon, :large_icon, :extension, :category
    attr_reader :proc

    def initialize(name, &blk)
      @menu_text = name.to_s
      @tooltip = name.to_s
      @proc = blk
      @dir = File.dirname(caller_locations(1, 1)[0].absolute_path.to_s)
    end

    def set_validation_proc(&_b); nil; end

    # Icon path as given, or relative to the file that created the command.
    def _icon
      i = (@large_icon || @small_icon).to_s
      return '' if i.empty?
      File.exist?(i) ? File.expand_path(i) : File.expand_path(i, @dir)
    end
  end

  class Toolbar
    include Enumerable
    attr_reader :name

    def initialize(name)
      @name = name.to_s
      @items = []
      @visible = false
    end

    def add_item(cmd)
      id = Host.register_proc(cmd.proc)
      @items << cmd
      Host.note('toolbar.item', 'toolbar' => @name, 'label' => cmd.menu_text, 'tooltip' => cmd.tooltip.to_s,
                                'icon' => cmd._icon, 'proc' => id)
      self
    end

    def add_separator
      Host.note('toolbar.separator', 'toolbar' => @name, 'label' => '', 'tooltip' => '', 'icon' => '')
      self
    end

    def show; @visible = true; Host.note('toolbar.visible', 'toolbar' => @name, 'visible' => true); nil; end
    alias restore show
    def hide; @visible = false; Host.note('toolbar.visible', 'toolbar' => @name, 'visible' => false); nil; end
    def visible?; @visible; end
    def get_last_state; TB_NEVER_SHOWN; end
    def each(&b); @items.each(&b); end
    def count; @items.length; end
    alias length count
    alias size count
  end

  def self.messagebox(message, type = MB_OK, _title = nil)
    Host.request('ui.messagebox', message.to_s, type.to_i)
  end

  # inputbox(prompts, defaults, title) or inputbox(prompts, defaults, lists, title).
  def self.inputbox(prompts, defaults = [], *rest)
    title = rest.length >= 2 ? rest[1] : (rest[0].is_a?(String) ? rest[0] : nil)
    lists = rest.length >= 2 ? rest[0] : (rest[0].is_a?(Array) ? rest[0] : [])
    prompts = Array(prompts).map(&:to_s)
    defaults = Array(defaults)
    shown = defaults.map { |d| d.is_a?(Float) ? SketchCAD.format_length(d) : d.to_s }
    r = Host.request('ui.inputbox', prompts, shown, Array(lists).map(&:to_s), title.to_s)
    return false unless r
    r.each_with_index.map do |v, i|
      d = defaults[i]
      case d
      when Integer then v.to_i
      when Float then (v.to_l rescue v.to_f)
      else v
      end
    end
  end

  def self.openpanel(title = '', dir = '', file = '')
    Host.request('ui.openpanel', title.to_s, dir.to_s, file.to_s)
  end

  def self.savepanel(title = '', dir = '', file = '')
    Host.request('ui.savepanel', title.to_s, dir.to_s, file.to_s)
  end

  def self.select_directory(options = {})
    Host.request('ui.select_directory', options[:title].to_s, options[:directory].to_s, '')
  end

  @timer = 0
  def self.start_timer(seconds, repeat = false, &blk)
    @timer += 1
    Host.note('timer.start', 'id' => @timer, 'seconds' => seconds.to_f, 'repeat' => repeat ? true : false,
                             'proc' => Host.register_proc(blk))
    @timer
  end

  def self.stop_timer(id)
    Host.note('timer.stop', 'id' => id.to_i)
    nil
  end

  def self.beep; nil; end
  def self.play_sound(_f); nil; end
  def self.refresh_inspectors; nil; end
  def self.show_inspector(_n); false; end
  def self.inspector_names; []; end
  def self.show_model_info(_p = nil); false; end
  def self.show_preferences(_p = nil); false; end
  def self.set_cursor(_id); true; end
  def self.create_cursor(*_); 0; end
  def self.scale_factor(*_); 1.0; end
  def self.toolbar_names; []; end
  def self.toolbar_visible?(_n); false; end
  def self.set_toolbar_visible(_n, _v); nil; end
  def self.open_external_file(_p); false; end
  def self.openURL(_u); Host.warn_once(:url, 'opening web pages from extensions is not supported yet'); false; end

  class HtmlDialog
    def initialize(*_); end
    def show; raise NotImplementedError, 'UI::HtmlDialog is not supported by SketchCAD yet'; end
    alias show_modal show
    def visible?; false; end
    def method_missing(*_); nil; end
    def respond_to_missing?(*_); true; end
  end

  class WebDialog < HtmlDialog; end

  class Notification
    def initialize(_ext = nil, message = '', *_)
      @message = message
    end

    attr_accessor :message
    def show; Sketchup.status_text = @message; STDERR.puts(@message); true; end
    def on_accept(*_, &_b); nil; end
    def on_dismiss(*_, &_b); nil; end
    def icon_name=(_n); nil; end
    def icon_tooltip=(_n); nil; end
  end
end

MB_OK = 0
MB_OKCANCEL = 1
MB_ABORTRETRYIGNORE = 2
MB_YESNOCANCEL = 3
MB_YESNO = 4
MB_RETRYCANCEL = 5
MB_MULTILINE = 16
IDOK = 1
IDCANCEL = 2
IDABORT = 3
IDRETRY = 4
IDIGNORE = 5
IDYES = 6
IDNO = 7
MF_ENABLED = 0
MF_GRAYED = 1
MF_DISABLED = 2
MF_CHECKED = 8
MF_UNCHECKED = 0
TB_VISIBLE = 1
TB_HIDDEN = 0
TB_NEVER_SHOWN = -1
