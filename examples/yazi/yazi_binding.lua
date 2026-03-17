---@meta

---@alias Area "pos" | "rect"

---@alias Renderable "line" | "text" | "list" | "bar" | "clear" | "border" | "gauge" | "table"

---@alias Error "io" | "fs" | "serde" | "custom"

---@class Bar
local Bar = {}

---@param area any?
---@return any
function Bar.area(area) end

---@param style Style
---@return Bar
function Bar.style(style) end

---@param edge Edge
---@return Bar
function Bar.edge(edge) end

---@param symbol string
---@return Bar
function Bar.symbol(symbol) end

---@class Border
local Border = {}

---@param area any?
---@return any
function Border.area(area) end

---@param style Style
---@return Border
function Border.style(style) end

---@param value integer
---@return Border
function Border.type(value) end

---@param line Line
---@param position integer?
---@return Border
function Border.title(line, position) end

---@param edge Edge
---@return Border
function Border.edge(edge) end

---@class Clear
local Clear = {}

---@param area any?
---@return any
function Clear.area(area) end

---@class Constraint
local Constraint = {}

---@class Gauge
local Gauge = {}

---@param area any?
---@return any
function Gauge.area(area) end

---@param style Style
---@return Gauge
function Gauge.style(style) end

---@param percent integer
---@return Gauge
function Gauge.percent(percent) end

---@param ratio number
---@return Gauge
function Gauge.ratio(ratio) end

---@param label Span
---@return Gauge
function Gauge.label(label) end

---@param style Style
---@return Gauge
function Gauge.gauge_style(style) end

---@class Layout
local Layout = {}

---@param value boolean
---@return Layout
function Layout.direction(value) end

---@param value integer
---@return Layout
function Layout.margin(value) end

---@param value integer
---@return Layout
function Layout.margin_h(value) end

---@param value integer
---@return Layout
function Layout.margin_v(value) end

---@param value Constraint[]
---@return Layout
function Layout.constraints(value) end

---@param value Rect
---@return table
function Layout:split(value) end

---@class Line
local Line = {}

---@param area any?
---@return any
function Line.area(area) end

---@param style Style
---@return Line
function Line.style(style) end

---@param value any
---@return any
function Line.fg(value) end

---@param value any
---@return any
function Line.bg(value) end

---@param remove boolean
---@return Line
function Line.bold(remove) end

---@param remove boolean
---@return Line
function Line.dim(remove) end

---@param remove boolean
---@return Line
function Line.italic(remove) end

---@param remove boolean
---@return Line
function Line.underline(remove) end

---@param remove boolean
---@return Line
function Line.blink(remove) end

---@param remove boolean
---@return Line
function Line.blink_rapid(remove) end

---@param remove boolean
---@return Line
function Line.reverse(remove) end

---@param remove boolean
---@return Line
function Line.hidden(remove) end

---@param remove boolean
---@return Line
function Line.crossed(remove) end

---@return Line
function Line.reset() end

---@return integer
function Line:width() end

---@param align Align
---@return Line
function Line.align(align) end

---@return boolean
function Line:visible() end

---@param t table
---@return Line
function Line.truncate(t) end

---@class List
local List = {}

---@param area any?
---@return any
function List.area(area) end

---@class Pad
---@field left integer (readonly)
---@field right integer (readonly)
---@field top integer (readonly)
---@field bottom integer (readonly)
local Pad = {}

---@class Pos
---@field ["1"] string (readonly)
---@field x integer (readonly)
---@field y integer (readonly)
---@field w integer (readonly)
---@field h integer (readonly)
local Pos = {}

---@param pad Pad
---@return Pos
function Pos.pad(pad) end

---@class Rect
---@field x integer
---@field y integer
---@field w integer
---@field h integer
---@field left integer (readonly)
---@field right integer (readonly)
---@field top integer (readonly)
---@field bottom integer (readonly)
local Rect = {}

---@param pad Pad
---@return Rect
function Rect:pad(pad) end

---@param p1 Rect
---@return boolean
function Rect:contains(p1) end

---@class Row
local Row = {}

---@param style Style
---@return Row
function Row.style(style) end

---@param value integer
---@return Row
function Row.height(value) end

---@param value integer
---@return Row
function Row.margin_t(value) end

---@param value integer
---@return Row
function Row.margin_b(value) end

---@class Span
local Span = {}

---@param style Style
---@return Span
function Span.style(style) end

---@param value any
---@return any
function Span.fg(value) end

---@param value any
---@return any
function Span.bg(value) end

---@param remove boolean
---@return Span
function Span.bold(remove) end

---@param remove boolean
---@return Span
function Span.dim(remove) end

---@param remove boolean
---@return Span
function Span.italic(remove) end

---@param remove boolean
---@return Span
function Span.underline(remove) end

---@param remove boolean
---@return Span
function Span.blink(remove) end

---@param remove boolean
---@return Span
function Span.blink_rapid(remove) end

---@param remove boolean
---@return Span
function Span.reverse(remove) end

---@param remove boolean
---@return Span
function Span.hidden(remove) end

---@param remove boolean
---@return Span
function Span.crossed(remove) end

---@return Span
function Span.reset() end

---@return boolean
function Span:visible() end

---@param t table
---@return Span
function Span.truncate(t) end

---@class Table
local Table = {}

---@param area any?
---@return any
function Table.area(area) end

---@param header Row
---@return Table
function Table.header(header) end

---@param footer Row
---@return Table
function Table.footer(footer) end

---@param widths Constraint[]
---@return Table
function Table.widths(widths) end

---@param spacing integer
---@return Table
function Table.spacing(spacing) end

---@param idx integer?
---@return Table
function Table.row(idx) end

---@param idx integer?
---@return Table
function Table.col(idx) end

---@param style Style
---@return Table
function Table.style(style) end

---@param style Style
---@return Table
function Table.row_style(style) end

---@param style Style
---@return Table
function Table.col_style(style) end

---@param style Style
---@return Table
function Table.cell_style(style) end

---@class Text
local Text = {}

---@param area any?
---@return any
function Text.area(area) end

---@param style Style
---@return Text
function Text.style(style) end

---@param value any
---@return any
function Text.fg(value) end

---@param value any
---@return any
function Text.bg(value) end

---@param remove boolean
---@return Text
function Text.bold(remove) end

---@param remove boolean
---@return Text
function Text.dim(remove) end

---@param remove boolean
---@return Text
function Text.italic(remove) end

---@param remove boolean
---@return Text
function Text.underline(remove) end

---@param remove boolean
---@return Text
function Text.blink(remove) end

---@param remove boolean
---@return Text
function Text.blink_rapid(remove) end

---@param remove boolean
---@return Text
function Text.reverse(remove) end

---@param remove boolean
---@return Text
function Text.hidden(remove) end

---@param remove boolean
---@return Text
function Text.crossed(remove) end

---@return Text
function Text.reset() end

---@param align Align
---@return Text
function Text.align(align) end

---@param wrap Wrap
---@return Text
function Text.wrap(wrap) end

---@param x integer
---@param y integer
---@return Text
function Text.scroll(x, y) end

---@return integer?
function Text:max_width() end

---@class Access
local Access = {}

---@param append boolean
---@return Access
function Access.append(append) end

---@param create boolean
---@return Access
function Access.create(create) end

---@param create_new boolean
---@return Access
function Access.create_new(create_new) end

---@async
---@param url UserDataRef
---@return any Nil
---@return Error Io
function Access:open(url) end

---@param read boolean
---@return Access
function Access.read(read) end

---@param truncate boolean
---@return Access
function Access.truncate(truncate) end

---@param write boolean
---@return Access
function Access.write(write) end

---@class SizeCalculator
---@field cha Cha (readonly)
local SizeCalculator = {}

---@async
---@return any Nil
---@return Error Io
function SizeCalculator:recv() end

---@class Cha
---@field mode integer (readonly)
---@field is_dir boolean (readonly)
---@field is_hidden boolean (readonly)
---@field is_link boolean (readonly)
---@field is_orphan boolean (readonly)
---@field is_dummy boolean (readonly)
---@field is_block boolean (readonly)
---@field is_char boolean (readonly)
---@field is_fifo boolean (readonly)
---@field is_sock boolean (readonly)
---@field is_exec boolean (readonly)
---@field is_sticky boolean (readonly)
---@field len integer (readonly)
---@field atime number? (readonly)
---@field btime number? (readonly)
---@field ctime number? (readonly)
---@field mtime number? (readonly)
---@field dev integer (readonly)
---@field uid integer (readonly)
---@field gid integer (readonly)
---@field nlink integer (readonly)
local Cha = {}

---@param long boolean?
---@return string
function Cha:hash(long) end

---@return Value
function Cha:perm() end

---@class MpscTx
local MpscTx = {}

---@async
---@param value any
---@return boolean
---@return Error custom
function MpscTx:send(value) end

---@class MpscRx
local MpscRx = {}

---@async
---@return any value
---@return boolean
function MpscRx:recv() end

---@class MpscUnboundedTx
local MpscUnboundedTx = {}

---@param value any
---@return boolean
---@return Error custom
function MpscUnboundedTx:send(value) end

---@class MpscUnboundedRx
local MpscUnboundedRx = {}

---@async
---@return any value
---@return boolean
function MpscUnboundedRx:recv() end

---@class OneshotTx
local OneshotTx = {}

---@param value any
---@return boolean
---@return Error custom
function OneshotTx:send(value) end

---@class OneshotRx
local OneshotRx = {}

---@async
---@return any Nil
---@return Error custom
function OneshotRx:recv() end

---@class ChordCow
local ChordCow = {}

---@class Color
local Color = {}

---@class Composer
local Composer = {}

---@param key string
---@return any
function Composer:__index(key) end

---@param key string
---@param value any
function Composer:__newindex(key, value) end

---@class Error
---@field code integer? (readonly)
---@field kind string? (readonly)
local Error = {}

---@return string
function Error:__tostring() end

---@param lhs any
---@param rhs any
---@return string
function Error.__concat(lhs, rhs) end

---@class Fd
local Fd = {}

---@async
---@return boolean
---@return Error Io
function Fd:flush() end

---@async
---@param len integer
---@return any Nil
---@return Error Io
function Fd:read(len) end

---@async
---@param src string
---@return boolean
---@return Error Io
function Fd:write_all(src) end

---@class File
---@field cha Cha (readonly)
---@field url string (readonly)
---@field link_to Path? (readonly)
---@field name string? (readonly)
---@field path Path (readonly)
---@field cache Path? (readonly)
local File = {}

---@return integer
function File:hash() end

---@return Icon?
function File:icon() end

---@class Handle
local Handle = {}

function Handle:abort() end

---@class Icon
---@field text string (readonly)
---@field style Style (readonly)
local Icon = {}

---@class Id
---@field value integer (readonly)
local Id = {}

---@class ImageInfo
---@field w integer (readonly)
---@field h integer (readonly)
---@field ori integer? (readonly)
---@field format ImageFormat (readonly)
---@field color ImageColor (readonly)
local ImageInfo = {}

---@class ImageFormat
local ImageFormat = {}

---@return string
function ImageFormat:__tostring() end

---@class ImageColor
local ImageColor = {}

---@return string
function ImageColor:__tostring() end

---@class InputRx
local InputRx = {}

---@async
---@return string?
---@return integer
function InputRx:recv() end

---@class Iter
local Iter = {}

---@return integer
function Iter:__len() end

---@return function
---@return any
function Iter.__pairs() end

---@class Layer
local Layer = {}

---@return string
function Layer:__tostring() end

---@class MouseEvent
---@field x integer (readonly)
---@field y integer (readonly)
---@field is_left boolean (readonly)
---@field is_right boolean (readonly)
---@field is_middle boolean (readonly)
local MouseEvent = {}

---@class Path
---@field ext string? (readonly)
---@field name string? (readonly)
---@field parent Path? (readonly)
---@field stem string? (readonly)
---@field is_absolute boolean (readonly)
---@field has_root boolean (readonly)
local Path = {}

---@param child any
---@return boolean
function Path:ends_with(child) end

---@param other any
---@return Path
function Path:join(other) end

---@param base any
---@return boolean
function Path:starts_with(base) end

---@param base any
---@return Path?
function Path:strip_prefix(base) end

---@param rhs string
---@return string
function Path:__concat(rhs) end

---@param other UserDataRef
---@return boolean
function Path:__eq(other) end

---@return string
function Path:__tostring() end

---@class Permit
local Permit = {}

---@async
function Permit:drop() end

---@class Scheme
---@field kind string (readonly)
---@field cache Path? (readonly)
---@field is_virtual boolean (readonly)
local Scheme = {}

---@class FolderStage
local FolderStage = {}

---@return boolean
---@return Error Fs
function FolderStage:__call() end

---@class Style
local Style = {}

---@param value any
---@return any
function Style.fg(value) end

---@param value any
---@return any
function Style.bg(value) end

---@param remove boolean
---@return Style
function Style.bold(remove) end

---@param remove boolean
---@return Style
function Style.dim(remove) end

---@param remove boolean
---@return Style
function Style.italic(remove) end

---@param remove boolean
---@return Style
function Style.underline(remove) end

---@param remove boolean
---@return Style
function Style.blink(remove) end

---@param remove boolean
---@return Style
function Style.blink_rapid(remove) end

---@param remove boolean
---@return Style
function Style.reverse(remove) end

---@param remove boolean
---@return Style
function Style.hidden(remove) end

---@param remove boolean
---@return Style
function Style.crossed(remove) end

---@return Style
function Style.reset() end

---@return Lua
function Style:raw() end

---@param style Style
---@return Style
function Style.patch(style) end

---@class Url
---@field path Path (readonly)
---@field name string? (readonly)
---@field stem string? (readonly)
---@field ext string? (readonly)
---@field urn Path (readonly)
---@field base string? (readonly)
---@field parent string? (readonly)
---@field scheme Scheme (readonly)
---@field domain string? (readonly)
---@field cache Path? (readonly)
---@field is_regular boolean (readonly)
---@field is_search boolean (readonly)
---@field is_archive boolean (readonly)
---@field is_absolute boolean (readonly)
---@field has_root boolean (readonly)
local Url = {}

---@param child any
---@return boolean
function Url:ends_with(child) end

---@param long boolean?
---@return string
function Url:hash(long) end

---@param other any
---@return string
function Url:join(other) end

---@param base any
---@return boolean
function Url:starts_with(base) end

---@param base any
---@return Path?
function Url:strip_prefix(base) end

---@param domain string
---@return string
function Url.into_search(domain) end

---@param other UserDataRef
---@return boolean
function Url:__eq(other) end

---@return string
function Url:__tostring() end

---@param rhs string
---@return string
function Url:__concat(rhs) end
