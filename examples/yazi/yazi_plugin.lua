---@meta

---@alias FetchState "bool" | "vec"

---@class Child
local Child = {}

---@return integer?
function Child:id() end

---@async
---@param len integer
---@return string
---@return integer
function Child:read(len) end

---@async
---@return string
---@return integer event
function Child:read_line() end

---@async
---@param options table<string, integer>
---@return string
---@return integer event
function Child:read_line_with(options) end

---@async
---@param src string
---@return boolean
---@return Error?
function Child:write_all(src) end

---@async
---@return boolean
---@return Error?
function Child:flush() end

---@async
---@return ExitStatus | Status
---@return Error?
function Child:wait() end

---@async
---@return Output
---@return Error?
function Child.wait_with_output() end

---@async
---@return Status?
---@return Error?
function Child:try_wait() end

---@return boolean
---@return Error?
function Child:start_kill() end

---@return ChildStdin?
function Child:take_stdin() end

---@return ChildStdout?
function Child:take_stdout() end

---@return ChildStderr?
function Child:take_stderr() end

---@class Command
local Command = {}

---@param arg (string | string[])?
---@return Command | string[]
function Command.arg(arg) end

---@param dir string
---@return Command
function Command.cwd(dir) end

---@param key string
---@param value string
---@return Command
function Command.env(key, value) end

---@param stdio integer | ChildStdin | ChildStdout | ChildStderr
---@return Command
function Command.stdin(stdio) end

---@param stdio integer | ChildStdin | ChildStdout | ChildStderr
---@return Command
function Command.stdout(stdio) end

---@param stdio integer | ChildStdin | ChildStdout | ChildStderr
---@return Command
function Command.stderr(stdio) end

---@param max integer
---@return Command
function Command.memory(max) end

---@return Child
---@return Error?
function Command:spawn() end

---@async
---@return Output
---@return Error?
function Command:output() end

---@async
---@return ExitStatus | Status
---@return Error?
function Command:status() end

---@class Fetcher
---@field cmd string (readonly)
local Fetcher = {}

---@class Output
---@field status Status (readonly)
---@field stdout string (readonly)
---@field stderr string (readonly)
local Output = {}

---@class Preloader
---@field cmd string (readonly)
local Preloader = {}

---@class Previewer
---@field cmd string (readonly)
local Previewer = {}

---@class Spotter
---@field cmd string (readonly)
local Spotter = {}

---@class Status
---@field success boolean (readonly)
---@field code integer? (readonly)
local Status = {}

---@class Command
---@field NULL integer
---@field PIPED integer
---@field INHERIT integer
Command = {}

---@class fs
fs = {}

---@return Access
function fs.access() end

---@async
---@param url Url
---@return SizeCalculator
---@return Error?
function fs.calc_size(url) end

---@async
---@param url Url
---@param follow boolean?
---@return Cha
---@return Error?
function fs.cha(url, follow) end

---@async
---@param from Url
---@param to Url
---@return integer
---@return Error?
function fs.copy(from, to) end

---@async
---@param type string
---@param url Url
---@return boolean
---@return Error?
function fs.create(type, url) end

---@return Url
---@return Error?
function fs.cwd() end

---@param value string | UrlBuf
---@return Url
function fs.expand_url(value) end

---@param name string
---@param t table<string, Id | Url | UrlBuf | File[]> | table<string, Id | Cha | Url | UrlBuf> | table<string, Url | UrlBuf | table<Path, integer>>
---@return FilesOp
function fs.op(name, t) end

---@async
---@return table[]
function fs.partitions() end

---@async
---@param dir Url
---@param options table<string, string | integer | boolean>
---@return (Pattern | ReadDir | File[])?
---@return Error?
function fs.read_dir(dir, options) end

---@async
---@param type string
---@param url Url
---@return boolean
---@return Error?
function fs.remove(type, url) end

---@async
---@param from Url
---@param to Url
---@return boolean
---@return Error?
function fs.rename(from, to) end

---@async
---@param type string
---@param url Url
---@return UrlBuf | Url
---@return Error?
function fs.unique(type, url) end

---@async
---@param url Url
---@return string | UrlBuf | Url
---@return Error?
function fs.unique_name(url) end

---@async
---@param url Url
---@param data string
---@return boolean
---@return Error?
function fs.write(url, data) end

---@class ps
ps = {}

---@param kind string
---@param value (boolean | number | string | table)?
---@return Pubsub
function ps.pub(kind, value) end

---@param receiver Id
---@param kind string
---@param value (boolean | number | string | table)?
---@return Pubsub
function ps.pub_to(receiver, kind, value) end

---@param kind string
---@param f function
function ps.sub(kind, f) end

---@param kind string
---@param f function
function ps.sub_remote(kind, f) end

---@param kind string
---@return boolean
function ps.unsub(kind) end

---@param kind string
---@return boolean
function ps.unsub_remote(kind) end

---@class rt
---@field args rt__Args
---@field term rt__Term
---@field mgr rt__Mgr
---@field plugin rt__Plugin
---@field preview rt__Preview
---@field tasks rt__Tasks
rt = {}

---@class rt__Args
---@field entries Url[]
---@field cwd_file Url?
---@field chooser_file Url?
rt__Args = {}

---@class rt__Mgr
---@field ratio SyncCell
---@field sort_by SyncCell
---@field sort_sensitive boolean
---@field sort_reverse boolean
---@field sort_dir_first boolean
---@field sort_translit boolean
---@field sort_fallback SyncCell
---@field linemode string
---@field show_hidden boolean
---@field show_symlink boolean
---@field scrolloff integer
---@field mouse_events SyncCell
rt__Mgr = {}

---@class rt__Plugin
rt__Plugin = {}

---@param file File
---@param mime string
---@return Fetcher[]
function rt__Plugin.fetchers(file, mime) end

---@param file File
---@param mime string
---@return Spotter?
function rt__Plugin.spotter(file, mime) end

---@param file File
---@param mime string
---@return Preloader[]
function rt__Plugin.preloaders(file, mime) end

---@param file File
---@param mime string
---@return Previewer?
function rt__Plugin.previewer(file, mime) end

---@class rt__Preview
---@field wrap Wrap
---@field tab_size integer
---@field max_width integer
---@field max_height integer
---@field cache_dir string
---@field image_delay integer
---@field image_filter string
---@field image_quality integer
---@field ueberzug_scale number
---@field ueberzug_offset any
rt__Preview = {}

---@class rt__Tasks
---@field file_workers integer
---@field plugin_workers integer
---@field fetch_workers integer
---@field preload_workers integer
---@field process_workers integer
---@field bizarre_retry integer
---@field image_alloc integer
---@field image_bound integer[]
---@field suppress_preload boolean
rt__Tasks = {}

---@class rt__Term
---@field light boolean
rt__Term = {}

---@return number?
---@return number?
function rt__Term.cell_size() end

---@class th
---@field app th__App
---@field mgr th__Mgr
---@field tabs th__Tabs
---@field mode th__Mode
---@field indicator th__Indicator
---@field status th__Status
---@field which th__Which
---@field confirm th__Confirm
---@field spot th__Spot
---@field notify th__Notify
---@field pick th__Pick
---@field input th__Input
---@field cmp th__Cmp
---@field tasks th__Tasks
---@field help th__Help
th = {}

---@class th__App
---@field overall Style
th__App = {}

---@class th__Cmp
---@field border Style
---@field active Style
---@field inactive Style
---@field icon_file string
---@field icon_folder string
---@field icon_command string
th__Cmp = {}

---@class th__Confirm
---@field border Style
---@field title Style
---@field body Style
---@field list Style
---@field btn_yes Style
---@field btn_no Style
---@field btn_labels string[]
th__Confirm = {}

---@class th__Help
---@field on Style
---@field run Style
---@field desc Style
---@field hovered Style
---@field footer Style
th__Help = {}

---@class th__Indicator
---@field parent Style
---@field current Style
---@field preview Style
---@field padding th__Indicator__Padding
th__Indicator = {}

---@class th__Indicator__Padding
---@field open string
---@field close string
th__Indicator__Padding = {}

---@class th__Input
---@field border Style
---@field title Style
---@field value Style
---@field selected Style
th__Input = {}

---@class th__Mgr
---@field cwd Style
---@field find_keyword Style
---@field find_position Style
---@field symlink_target Style
---@field marker_copied Style
---@field marker_cut Style
---@field marker_marked Style
---@field marker_selected Style
---@field marker_symbol string
---@field count_copied Style
---@field count_cut Style
---@field count_selected Style
---@field border_symbol string
---@field border_style Style
---@field syntect_theme Url
th__Mgr = {}

---@class th__Mode
---@field normal_main Style
---@field normal_alt Style
---@field select_main Style
---@field select_alt Style
---@field unset_main Style
---@field unset_alt Style
th__Mode = {}

---@class th__Notify
---@field title_info Style
---@field title_warn Style
---@field title_error Style
---@field icon_info string
---@field icon_warn string
---@field icon_error string
th__Notify = {}

---@class th__Pick
---@field border Style
---@field active Style
---@field inactive Style
th__Pick = {}

---@class th__Spot
---@field border Style
---@field title Style
---@field tbl_col Style
---@field tbl_cell Style
th__Spot = {}

---@class th__Status
---@field overall Style
---@field sep_left th__Status__SepLeft
---@field sep_right th__Status__SepRight
---@field perm_sep Style
---@field perm_type Style
---@field perm_read Style
---@field perm_write Style
---@field perm_exec Style
---@field progress_label Style
---@field progress_normal Style
---@field progress_error Style
th__Status = {}

---@class th__Status__SepLeft
---@field open string
---@field close string
th__Status__SepLeft = {}

---@class th__Status__SepRight
---@field open string
---@field close string
th__Status__SepRight = {}

---@class th__Tabs
---@field active Style
---@field inactive Style
---@field sep_inner th__Tabs__SepInner
---@field sep_outer th__Tabs__SepOuter
th__Tabs = {}

---@class th__Tabs__SepInner
---@field open string
---@field close string
th__Tabs__SepInner = {}

---@class th__Tabs__SepOuter
---@field open string
---@field close string
th__Tabs__SepOuter = {}

---@class th__Tasks
---@field border Style
---@field title Style
---@field hovered Style
th__Tasks = {}

---@class th__Which
---@field cols integer
---@field mask Style
---@field cand Style
---@field rest Style
---@field desc Style
---@field separator string
---@field separator_style Style
th__Which = {}

---@class ui
ui = {}

---@param s string
---@return Rect
function ui.area(s) end

---@param s string
---@param opts table<string, (integer | Wrap | boolean)?>
---@return integer
function ui.height(s, opts) end

---@async
---@return self.type_id()
function ui.hide() end

---@param s string
---@return string
function ui.printable(s) end

---@param c table<string, string | Rect>
---@return table | any[] | table<any, any>
function ui.redraw(c) end

function ui.render() end

---@param s string
---@param t table<string, integer | boolean>
---@return string
function ui.truncate(s, t) end

---@param v string | Line | Span
---@return integer
function ui.width(v) end

---@class ya
ya = {}

---@param type string
---@return Id
function ya.id(type) end

---@param ud Fd | ChildStdin | ChildStdout | ChildStderr
function ya.drop(ud) end

---@param t table<string, File | integer>
---@return Url?
function ya.file_cache(t) end

---@param name string
---@param args table<integer | string, (boolean | number | string | table)?>
---@return Call
function ya.emit(name, args) end

---@param name string
---@param args table<integer | string, (boolean | number | string | table)?>
function ya.mgr_emit(name, args) end

---@async
---@param url Url
---@return ImageInfo
---@return Error? custom
function ya.image_info(url) end

---@async
---@param url Url
---@param rect Rect
---@return Rect
---@return Error? custom
function ya.image_show(url, rect) end

---@async
---@param src Url
---@param dist Url
---@return boolean
---@return Error? custom
function ya.image_precache(src, dist) end

---@async
---@param value (boolean | number | string | table)?
---@return string
---@return Error?
function ya.json_encode(value) end

---@async
---@param s string
---@return (boolean | number | string | table)?
---@return Error?
function ya.json_decode(s) end

---@async
---@param t table<string, boolean>
---@return integer?
function ya.which(t) end

---@async
---@param t table<string, boolean | string | Position | number>
---@return (string | InputRx)?
---@return integer?
function ya.input(t) end

---@async
---@param t table<string, (Position | Line | Text)?>
---@return boolean
function ya.confirm(t) end

---@param opt PushOpt
---@return NotifyProxy
function ya.notify(opt) end

---@param values MultiValue
function ya.dbg(values) end

---@param values MultiValue
function ya.err(values) end

---@async
---@param t PreviewLock
---@return (Text | string)?
---@return integer?
function ya.preview_code(t) end

---@async
---@param t PreviewLock
---@param value (Renderable[] | Renderable | Error)?
function ya.preview_widget(t, value) end

---@return table<string, integer>
function ya.proc_info() end

---@param t table
---@param table Table
function ya.spot_table(t, table) end

---@param t SpotLock
---@param widgets Renderable[]
function ya.spot_widgets(t, widgets) end

---@param f function
---@return fun(p1: MultiValue): any...
function ya.co(f) end

---@param f fun(p1: any...): any...
---@return fun(p1: MultiValue): any...
function ya.sync(f) end

---@param p1 function
function ya.async(p1) end

---@param type string
---@param buffer integer?
---@return BorrowedBytes | MpscTx | MpscUnboundedTx | OneshotTx
---@return (integer | MpscRx | MpscUnboundedRx | OneshotRx)?
function ya.chan(type, buffer) end

---@async
---@param fns Variadic<Function>
---@return any...
function ya.join(fns) end

---@async
---@param _futs MultiValue
function ya.select(_futs) end

---@return string
function ya.target_os() end

---@return string
function ya.target_family() end

---@async
---@param s string
---@return string
function ya.hash(s) end

---@param s string
---@param unix boolean?
---@return string
function ya.quote(s, unix) end

---@async
---@param text string?
---@return string?
function ya.clipboard(text) end

---@return number?
function ya.time() end

---@async
---@param secs number
function ya.sleep(secs) end

---@async
---@param id string
---@return table<string, table>
function require(id) end
