-- Maschine MK2: notes use Pads MIDI, controls and feedback use Controls MIDI.
local M = {}
local sent, values, held = {}, {}, {}
local browsing, sampling, wheel, user_content = false, false, nil, false
local step_mode, step_page, duplicate_from, browser_kind = false, 0, nil, 1
local view_mode, pinned, mode_before, mode_down, mode_started, pin_touched = nil, nil, nil, nil, 0, false
local pattern_bank, scene_bank, knob_acc = 0, 0, {}
local sample_pages, sample_turns, sample_asked = {1,1,1,1}, {}, {}
local sample_function, sample_slice, sample_bank, sample_identity = 1, 1, 0, nil
local asked_follow, sample_take

local function light(id, r, g, b)
  r, g, b = math.floor(math.max(0, math.min(127, r))), math.floor(math.max(0, math.min(127, g))), math.floor(math.max(0, math.min(127, b)))
  local key = r .. ":" .. g .. ":" .. b
  if sent[id] == key then return end
  sent[id] = key
  deckard.midi_send("controls", { 0xf0, 0x7d, 0x44, 0x4b, id, r, g, b, 0xf7 })
end

local function rgb(id, color, level)
  color = color or { 255, 255, 255 }
  light(id, color[1] * level / 2, color[2] * level / 2, color[3] * level / 2)
end

-- Both LCDs use pixel primitives. Each non-overlapping region is repainted only
-- when its drawing changes, so a play cursor or a knob leaves its neighbours alone.
local regions, layout, drawing, wave_columns = {}, nil, nil, {}
local function ascii(value)
  local text = tostring(value or "")
  if text:find("[\128-\255]") then
    for from, to in pairs({["á"]="a",["à"]="a",["â"]="a",["ã"]="a",["ä"]="a",["é"]="e",["ê"]="e",["í"]="i",["ó"]="o",["ô"]="o",["õ"]="o",["ö"]="o",["ú"]="u",["ü"]="u",["ç"]="c",["Á"]="A",["À"]="A",["Â"]="A",["Ã"]="A",["É"]="E",["Ê"]="E",["Í"]="I",["Ó"]="O",["Ô"]="O",["Õ"]="O",["Ú"]="U",["Ç"]="C",["–"]="-",["—"]="-",["−"]="-",["∞"]="inf"}) do
      text = text:gsub(from, to)
    end
  end
  return text:gsub("[^ -~]", "?")
end
local function clamp(value, lo, hi)
  return math.max(lo, math.min(hi, value))
end
local function send_graphic(bytes)
  if drawing then drawing[#drawing + 1] = bytes else deckard.midi_send("controls", bytes) end
end
local function rect(screen, x, y, w, h, style)
  x, y, w, h = math.floor(x), math.floor(y), math.floor(w), math.floor(h)
  if w <= 0 or h <= 0 then return end
  send_graphic({0xf0,0x7d,0x44,0x47,1,screen,x % 128,x // 128,y % 128,y // 128,w % 128,w // 128,h % 128,h // 128,style or 0,0xf7})
end
local function text(screen, x, y, value, width, scale)
  scale = scale or 1
  local label = ascii(value):sub(1, math.max(0, math.floor(((width or 255 - x) + scale) / (6 * scale))))
  if #label == 0 then return end
  local bytes = {0xf0,0x7d,0x44,0x47,2,screen,x % 128,x // 128,y % 128,y // 128,scale}
  for i = 1, #label do bytes[#bytes + 1] = label:byte(i) end
  bytes[#bytes + 1] = 0xf7
  send_graphic(bytes)
end
local function centered(screen, x, y, w, value, scale)
  scale = scale or 1
  local label = ascii(value):sub(1, math.floor((w + scale) / (6 * scale)))
  text(screen, x + math.floor((w - (#label * 6 - 1) * scale) / 2), y, label, w, scale)
end
local function region(screen, key, x, y, w, h, draw)
  drawing = {}
  draw()
  local packets, signature = drawing, {}
  drawing = nil
  for i, bytes in ipairs(packets) do signature[i] = string.char(table.unpack(bytes)) end
  signature = table.concat(signature)
  key = screen .. ":" .. key
  if regions[key] == signature then return end
  regions[key] = signature
  rect(screen, x, y, w, h, 5)
  for _, bytes in ipairs(packets) do deckard.midi_send("controls", bytes) end
end
local function begin_layout(name)
  if layout == name then return end
  layout, regions, wave_columns = name, {}, {}
  for screen = 0, 1 do deckard.midi_send("controls", {0xf0,0x7d,0x44,0x47,0,screen,0xf7}) end
end
local function button_bar(screen, buttons)
  for i = 1, 4 do
    local b, x = buttons[i] or {}, (i - 1) * 64
    region(screen, "button" .. i, x, 0, 63, 11, function()
      if not b.label or b.label == "" then return end
      rect(screen, x, 0, 63, 10, b.enabled == false and 2 or 0)
      centered(screen, x + 1, 1, 61, b.label)
      if b.selected then rect(screen, x + 1, 1, 61, 8, 4) end
    end)
  end
end
local function info(screen, left, right)
  region(screen, "info", 0, 11, 255, 10, function()
    right = ascii(right)
    local rw = #right * 6
    text(screen, 1, 12, left, math.max(0, 251 - rw))
    text(screen, math.max(1, 255 - rw), 12, right, rw)
    rect(screen, 0, 20, 255, 1, 3)
  end)
end
local function parameter(screen, column, knob, y, h)
  local x = (column - 1) * 64
  region(screen, "param" .. column, x, y, 63, h, function()
    if column > 1 then rect(screen, x, y + 1, 1, h - 1, 1) end
    if not knob then return end
    text(screen, x + 3, y + 1, knob.name, 59)
    local value = ascii(knob.text or "")
    local large = h >= 30 and #value <= 5
    centered(screen, x + 2, y + (h >= 30 and 13 or 10), 60, value, large and 2 or 1)
    if h >= 30 and knob.value ~= nil then
      rect(screen, x + 3, y + h - 9, 57, 6, 0)
      rect(screen, x + 5, y + h - 7, math.floor(clamp(knob.value, 0, 1) * 53 + .5), 2, 1)
      if knob.auto_mod or knob.step_mod then text(screen, x + 3, y + h - 18, knob.auto_mod or knob.step_mod, 57) end
    end
  end)
end
local function meter(screen, key, y, levels, threshold)
  region(screen, key, 0, y, 255, 11, function()
    local db = 20 * math.log(math.max(levels or 0, .000001), 10)
    local fill = math.floor(clamp((db + 60) / 60, 0, 1) * 224)
    text(screen, 1, y + 1, key == "meterL" and "L" or "R", 7)
    rect(screen, 12, y + 1, 228, 7, 0)
    rect(screen, 14, y + 3, fill, 3, 1)
    for i = 1, 3 do rect(screen, 14 + i * 56, y + 8, 1, 2, 1) end
    if threshold then rect(screen, 14 + math.floor(clamp((threshold + 60) / 60, 0, 1) * 224), y, 1, 10, 4) end
    rect(screen, 244, y + 1, 8, 7, (levels or 0) >= 1 and 1 or 0)
  end)
end

local function relative(cc, value)
  local old = values[cc]
  values[cc] = value
  if old == nil then return 0 end
  return (value - old + 64) % 128 - 64
end

local function steps(delta, scale)
  if delta == 0 then return 0 end
  return (delta > 0 and 1 or -1) * math.max(1, math.floor(math.abs(delta) / scale + .5))
end

local function bank(s)
  return math.floor(((s.group or 1) - 1) / 8) * 8
end

-- Keep rapid presses and manual paging consistent until the app acknowledges
-- FOLLOW. A later change from the app becomes authoritative immediately.
local function following(s)
  if asked_follow then
    if s.follow ~= asked_follow.shown then asked_follow = nil
    else return asked_follow.value end
  end
  return s.follow == true
end
local function set_follow(on, s)
  asked_follow = {shown=s.follow, value=on}
  deckard.follow(on)
end

local function sync_mode(s)
  local mode = view_mode == "pattern" and 2 or view_mode == "scene" and 3 or step_mode and 1 or 0
  if sampling then
    local tab = ((s or deckard.state()).sampling or {}).tab or 1
    mode = (tab == 1 or tab == 3) and 1 or 0
  end
  light(64, mode, 0, 0)
end

local function set_step(on)
  if on and browsing then browsing = false; deckard.library(false) end
  if on and sampling then sampling = false; deckard.sampling(false) end
  step_mode, view_mode, pinned = on, nil, nil
  sync_mode()
end

-- Follow the same grid origin and pattern limits as Deckard's native Step page.
local function step_grid(s)
  local length = math.max(1, s.grid and s.grid.step_ticks or 240)
  local start = s.pattern_start or 0
  local first = math.ceil(start / length) * length
  local finish = start + math.max(1, s.pattern_length or 3840)
  local last_page = math.max(0, math.ceil((finish - first) / (16 * length)) - 1)
  return first, finish, length, last_page
end

local function step_at(s, n)
  local first, finish, length = step_grid(s)
  local at = first + (step_page * 16 + n - 1) * length
  return at, length, at < finish
end

local function step_active(s, at, length)
  for _, note in ipairs(s.notes or {}) do
    if note.sound == s.sound and note.position >= at and note.position < at + length then return true end
  end
  return false
end

local function step_page_move(s, delta)
  if delta ~= 0 and following(s) then set_follow(false, s) end
  local _, _, _, last_page = step_grid(s)
  step_page = math.max(0, math.min(last_page, step_page + delta))
end

local function pattern_at(group, place)
  for index, pattern in ipairs(group and group.patterns or {}) do
    if (pattern.slot or index) == place then return index, pattern end
  end
end

local function mode_button(name, down)
  if down then
    if browsing then browsing = false; deckard.library(false) end
    if sampling then sampling = false; deckard.sampling(false) end
    mode_down, mode_started, pin_touched = name, deckard.now(), false
    if view_mode == name and pinned == name then
      mode_before = nil -- a second tap closes the pinned view
    else
      mode_before, view_mode, pinned = view_mode, name, nil
      sync_mode()
    end
  elseif mode_down == name then
    if pin_touched then
      if pinned ~= name then view_mode = mode_before end
    elseif pinned == name then
      view_mode, pinned = mode_before, nil
    elseif deckard.now() - mode_started < .35 then
      view_mode, pinned = name, name -- a short click stays visible
    else
      view_mode = mode_before
    end
    mode_down = nil
    sync_mode()
  end
end

local function pad_action(n, s, velocity)
  local group = s.groups[s.group]
  if held[33] then
    if group and group.sounds and group.sounds[n] then deckard.mute_sound(n) end
  elseif held[31] then
    if group and group.sounds and group.sounds[n] then deckard.solo_sound(n) end
  elseif held[30] then
    if group and group.sounds and group.sounds[n] then deckard.focus_sound(n) end
  elseif held[29] then
    if group and group.sounds and group.sounds[n] then
      if duplicate_from then deckard.copy_sound(duplicate_from, n); duplicate_from = nil
      else duplicate_from = n; deckard.message("Duplicar: selecione o pad de destino") end
    end
  elseif held[2] then
    if group and group.sounds and group.sounds[n] then
      if held[49] then deckard.reset_sound(n) else deckard.erase_sound(n) end
    end
  elseif view_mode == "scene" then
    local scene = scene_bank * 16 + n
    if s.scenes and s.scenes[scene] then deckard.focus_scene(scene) end
  elseif view_mode == "pattern" then
    local place = pattern_bank * 16 + n
    local index = pattern_at(group, place)
    if index then deckard.pick_pattern(index)
    elseif group then deckard.new_pattern(s.group, place) end
  elseif step_mode then
    local at, length, inside = step_at(s, n)
    if inside then deckard.step(at, length, velocity) end
  elseif group and group.sounds and group.sounds[n] then
    deckard.focus_sound(n)
  end
end

local function wheel_turn(delta, s)
  local n = steps(delta, 8)
  if n == 0 then return end
  if browsing then
    deckard.browse_focus(n * (held[49] and 10 or 1))
  elseif view_mode == "pattern" then
    local group = s.groups[s.group]
    local index = math.max(1, math.min(#(group and group.patterns or {}), (s.pattern or 1) + n))
    if group and group.patterns and group.patterns[index] then deckard.pick_pattern(index) end
  elseif wheel == "tempo" then
    deckard.tempo(math.max(10, math.min(400, (s.tempo or 120) + n)))
  elseif wheel == "swing" then
    deckard.swing(math.max(0, math.min(100, (s.swing or 0) + n)))
  elseif wheel == "volume" then
    deckard.volume(math.max(0, math.min(1, (s.volume or .75) + n * .01)))
  else
    deckard.slot(n)
  end
end

local SAMPLE_TABS = {"RECORD", "EDIT", "SLICE", "ZONE"}
local SAMPLE_FUNCTIONS = {
  {"TRUNCATE",1}, {"NORMALIZE",2}, {"REVERSE",3}, {"FADE IN",4}, {"FADE OUT",5},
  {"FIX DC",6}, {"SILENCE",7}, {"CUT",8}, {"COPY",9}, {"PASTE",10}, {"DUPLICATE",11},
  {"STEMS","stems"}, {"AUTOLOOP","auto_loop"},
}
local GRID_NAMES = {"1 BAR","1/2","1/4","1/8","1/16","1/32","1/64","1/128","1 BAR T","1/2T","1/4T","1/8T","1/16T","1/32T","1/64T"}
local function sample_state(s)
  return s.sampling or {tab=1, pages={}, slice={}, record={}, takes=0}
end
local function sample_busy(v)
  return v.record and (v.record.recording or v.record.waiting)
end
local function sample_ok(v)
  return v.sample and (v.sample.frames or 0) > 0
end
local function seconds(frames, rate)
  return string.format("%.3fs", (frames or 0) / math.max(1, rate or 1))
end
local function sample_knob_pages(v)
  if v.tab ~= 3 then return v.pages and v.pages[v.tab or 1] or {} end
  local k, sample = v.slice or {}, v.sample
  local mode = k["Slice.Mode"] and k["Slice.Mode"].text or ""
  local slicer = {k["Slice.Mode"]}
  if mode == "Detect" then slicer[2], slicer[3] = k["Slice.Density"], k["Slice.ZeroX"]
  elseif mode == "Split" then slicer[2] = k["Slice.Split"]
  elseif mode == "Grid" then slicer[2], slicer[3], slicer[4] = k["Slice.Grid"], k["Slice.BpmMode"], k["Slice.Bpm"] end
  local count = sample and #(sample.slices or {}) or 0
  sample_slice = clamp(sample_slice, 1, math.max(1, count))
  local edit = {{name="SLICE", text=count > 0 and (sample_slice .. "/" .. count) or "-/-", slice=true}}
  if count > 0 then
    local from, to = sample.slices[sample_slice], sample.slices[sample_slice + 1] or (sample.play or {})[2] or sample.frames
    edit[3] = {name="START", text=seconds(from, sample.rate), frame=from, move=sample_slice}
    edit[4] = {name="END", text=seconds(to, sample.rate), frame=to, move=sample.slices[sample_slice + 1] and sample_slice + 1 or nil}
  end
  return {
    {name="Slicer", knobs=slicer}, {name="Slice edit", knobs=edit},
    {name="Apply", knobs={[2]=k["Slice.Apply"], [3]=k["Slice.BpmMode"], [4]=k["Slice.Bpm"]}},
  }
end
local function sample_page(v)
  local pages = sample_knob_pages(v)
  local tab = v.tab or 1
  sample_pages[tab] = clamp(sample_pages[tab], 1, math.max(1, #pages))
  return pages[sample_pages[tab]] or {}, sample_pages[tab], #pages
end
local function sample_page_move(v, delta)
  local _, page, count = sample_page(v)
  sample_pages[v.tab or 1] = clamp(page + delta, 1, math.max(1, count))
end
local function sample_sync(s)
  local v = sample_state(s)
  local sample = v.sample
  local identity = (s.group or 1) .. ":" .. (s.sound or 1) .. ":" .. (sample and sample.name or "")
  if identity ~= sample_identity then
    sample_identity, sample_slice, sample_bank = identity, sample and sample.focus_slice or 1, 0
    sample_turns, sample_asked = {}, {}
  end
  if v.take ~= sample_take then
    sample_take = v.take
    if v.take then sample_bank = (v.take - 1) // 16 end
  end
  sample_bank = clamp(sample_bank, 0, math.max(0, math.ceil((v.takes or 0) / 16) - 1))
  local count = sample and #(sample.slices or {}) or 0
  sample_slice = clamp(sample_slice, 1, math.max(1, count))
end
local function sample_buttons(v)
  local tab, sample = v.tab or 1, v.sample
  local has = sample_ok(v)
  if tab == 1 then
    if sample_busy(v) then
      return {{label=v.record.waiting and "WAITING" or "STOP", selected=true}, {label="CANCEL"}, {}, {}}
    end
    if held[49] then
      local pages = math.max(1, math.ceil((v.takes or 0) / 16))
      return {{label="START"}, {label="DELETE", enabled=v.take ~= nil},
        {label="BANK -", enabled=sample_bank > 0}, {label="BANK +", enabled=sample_bank < pages - 1}}
    end
    return {{label="START"}, {label="DELETE", enabled=v.take ~= nil},
      {label="PREVIOUS", enabled=(v.takes or 0) > 0 and v.take ~= 1},
      {label="NEXT", enabled=(v.takes or 0) > 0 and v.take ~= v.takes}}
  elseif tab == 2 then
    local f = SAMPLE_FUNCTIONS[sample_function]
    local cancel = v.stems and v.stems.focused
    return {{label="FUNC -", enabled=has}, {label="FUNC +", enabled=has},
      {label="PREHEAR", enabled=has}, {label=cancel and "CANCEL" or "APPLY", enabled=has and (not v.stems or cancel)}}
  elseif tab == 3 then
    local count = sample and #(sample.slices or {}) or 0
    if held[49] then
      local bank_index = (sample_slice - 1) // 16
      return {{}, {}, {label="BANK -", enabled=bank_index > 0}, {label="BANK +", enabled=(bank_index + 1) * 16 < count}}
    end
    return {{label="SPLIT", enabled=count > 0}, {label="REMOVE", enabled=count > 0},
      {label="CLEAR ALL", enabled=count > 0}, {label="APPLY", enabled=count > 0}}
  end
  local count = sample and #(sample.zones or {}) or 0
  local zone = sample and sample.focus_zone or 1
  return {{label="REMOVE", enabled=count > 0}, {label="ADD", enabled=false},
    {label="PREVIOUS", enabled=zone > 1}, {label="NEXT", enabled=zone < count}}
end
local function sample_prehear(v)
  if not sample_ok(v) then return end
  local s = v.sample
  local range = s.selection or s.play or {0, s.frames}
  deckard.sampling_call("prehear", range[1], range[2])
end
local function sample_action(n, s)
  local v = sample_state(s)
  if n <= 4 then deckard.sampling_tab(n); return end
  local spec = sample_buttons(v)[n - 4]
  if not spec or not spec.label or spec.enabled == false then return end
  local tab, sample = v.tab or 1, v.sample
  if tab == 1 then
    if n == 5 then deckard.record_sample()
    elseif n == 6 then
      if sample_busy(v) then deckard.record_sample(true) else deckard.take(v.take, true) end
    elseif held[49] then sample_bank = sample_bank + (n == 7 and -1 or 1)
    elseif n == 7 then deckard.take(v.take and v.take - 1 or v.takes)
    elseif n == 8 then deckard.take(v.take and v.take + 1 or 1) end
  elseif tab == 2 then
    if n == 5 or n == 6 then sample_function = (sample_function + (n == 5 and -2 or 0)) % #SAMPLE_FUNCTIONS + 1
    elseif n == 7 then sample_prehear(v)
    elseif n == 8 then
      local action = SAMPLE_FUNCTIONS[sample_function][2]
      if v.stems and v.stems.focused then deckard.sampling_call("cancel_stems")
      elseif type(action) == "number" then deckard.sampling_call("edit", action)
      else deckard.sampling_call(action) end
    end
  elseif tab == 3 then
    if held[49] then sample_slice = clamp(sample_slice + (n == 7 and -16 or 16), 1, #(sample.slices or {}))
    elseif n == 5 then deckard.sampling_call("split_slice", sample_slice)
    elseif n == 6 then deckard.sampling_call("remove_slice", sample_slice)
    elseif n == 7 then deckard.sampling_call("clear_slices")
    elseif n == 8 then deckard.sampling_call("apply_slices") end
  else
    local zone = sample.focus_zone or 1
    if n == 5 then deckard.sampling_call("remove_zone", zone)
    elseif n == 7 or n == 8 then deckard.sampling_call("focus_zone", zone + (n == 7 and -1 or 1)) end
  end
end
local function sample_pad(n, s)
  local v = sample_state(s)
  if v.tab == 1 then
    local index = sample_bank * 16 + n
    if not sample_busy(v) and index <= (v.takes or 0) then deckard.take(index) end
    return true
  elseif v.tab == 3 then
    local sample = v.sample
    local index = ((sample_slice - 1) // 16) * 16 + n
    if sample and sample.slices and sample.slices[index] then
      sample_slice = index
      deckard.sampling_call("prehear", sample.slices[index], sample.slices[index + 1] or (sample.play or {})[2] or sample.frames)
    end
    return true
  end
  return false
end
local function sample_turn(key, knob, amount, send)
  local turn = math.max(.000001, knob.turn or .01)
  local total = (sample_turns[key] or 0) + amount * (held[49] and .1 or 1)
  local count = total >= 0 and math.floor(total / turn + 1e-9) or -math.floor(-total / turn + 1e-9)
  sample_turns[key] = total - count * turn
  if count == 0 then return end
  local asked = sample_asked[key]
  local from = asked and asked.shown == knob.value and asked.value or knob.value or 0
  local to = clamp(from + count * (knob.step or 1), knob.min or 0, knob.max or 1)
  if to == from then return end
  sample_asked[key] = {shown=knob.value, value=to}
  send(to)
end
local function sample_zoom(v, n, amount)
  if not sample_ok(v) or v.tab == 1 or sample_busy(v) then return end
  local sample = v.sample
  local view = sample.view or {0, sample.frames}
  local a, b = view[1], view[2]
  local asked = sample_asked.view
  if asked and asked.shown_a == a and asked.shown_b == b then a, b = asked.a, asked.b end
  local width = math.max(1, b - a)
  local fine = held[49] and .1 or 1
  if n == 5 then
    local next_width = clamp(width / 1.06 ^ (100 * amount * fine), math.min(sample.frames, 64), sample.frames)
    a = clamp((a + b - next_width) / 2, 0, sample.frames - next_width)
    b = a + next_width
  else
    a = clamp(a + 2 * width * amount * fine, 0, sample.frames - width)
    b = a + width
  end
  a, b = math.floor(a + .5), math.floor(b + .5)
  if a ~= view[1] or b ~= view[2] then
    sample_asked.view = {shown_a=view[1], shown_b=view[2], a=a, b=b}
    deckard.sampling_call("view", a, b)
  end
end
local function sample_knob(n, delta, s)
  if delta == 0 then return end
  local v = sample_state(s)
  if n == 5 or n == 6 then sample_zoom(v, n, delta / 127); return end
  if n > 4 or sample_busy(v) then return end
  local page, index = sample_page(v)
  local knob = page.knobs and page.knobs[n]
  if not knob then return end
  if knob.slice then
    local k = {value=sample_slice, min=1, max=math.max(1, #(v.sample.slices or {})), step=1, turn=.04}
    sample_turn("slice", k, delta / 127, function(to) sample_slice = to end)
  elseif knob.move then
    local sample = v.sample
    local view = sample.view or {0, sample.frames}
    local k = {value=knob.frame, min=0, max=sample.frames - 1,
      step=held[49] and 1 or math.max(1, math.floor((view[2] - view[1]) / 100)), turn=.01}
    sample_turn("move" .. knob.move, k, delta / 127, function(to) deckard.sampling_call("move_slice", knob.move, to) end)
  elseif knob.key then
    local target = v.tab == 3 and knob.index or n
    sample_turn((v.tab or 1) .. ":" .. index .. ":" .. knob.key, knob, delta / 127,
      function(to) deckard.sampling_knob(v.tab == 3 and 1 or index, target, to) end)
  end
end
local function sample_navigate(delta, s)
  local v = sample_state(s)
  if v.tab == 1 and not sample_busy(v) then
    local count = v.takes or 0
    if count > 0 then deckard.take(clamp((v.take or (delta > 0 and 0 or count + 1)) + delta, 1, count)) end
  elseif v.tab == 3 then
    local count = v.sample and #(v.sample.slices or {}) or 0
    sample_slice = clamp(sample_slice + delta, 1, math.max(1, count))
  elseif v.tab == 4 and v.sample then
    deckard.sampling_call("focus_zone", clamp((v.sample.focus_zone or 1) + delta, 1, math.max(1, #(v.sample.zones or {}))))
  elseif v.tab == 2 then sample_function = (sample_function + delta - 1) % #SAMPLE_FUNCTIONS + 1 end
end
local function change_grid(s, delta)
  deckard.grid("step", clamp((s.grid and s.grid.step or 4) + delta, 0, #GRID_NAMES - 1))
end
local function pattern_length_turn(s, delta, key)
  knob_acc[key] = (knob_acc[key] or 0) + delta
  local count = math.floor(math.abs(knob_acc[key]) / 8)
  if count == 0 then return end
  local direction = knob_acc[key] > 0 and 1 or -1
  knob_acc[key] = knob_acc[key] - direction * count * 8
  local bar = 960 * 4 / math.max(1, s.denominator or 4) * math.max(1, s.numerator or 4)
  local unit = held[49] and 240 or bar
  deckard.pattern_length(math.max(unit, (s.pattern_length or bar) + direction * count * unit))
end
local function browse_knob(n, delta, s)
  if delta == 0 then return end
  local b = s.browser or {}
  local amount = steps(delta, 2)
  if n == 1 then
    browser_kind = clamp((b.kind or browser_kind) + amount, 1, 7)
    deckard.browse_kind(browser_kind)
  elseif n == 2 then
    local products = b.products or {}
    local to = clamp((b.product or 0) + amount, 0, #products)
    deckard.browse_product(to > 0 and to or nil)
  elseif n >= 5 and n <= 7 then
    local a = b.attributes and b.attributes[n - 4] or {}
    local to = clamp((a.chosen or 0) + amount, 0, #(a.values or {}))
    deckard.browse_attribute(n - 4, to > 0 and to or nil)
  elseif n == 8 then deckard.browse_focus(amount * (held[49] and 20 or 1)) end
end

local function group_label(s)
  local n = (s.group or 1) - 1
  return string.char(65 + n % 8) .. (n // 8 + 1)
end
local function grid_label(s)
  local grid = s.grid or {}
  if grid.step ~= nil then return GRID_NAMES[grid.step + 1] or "CUSTOM" end
  local ticks = grid.step_ticks or 240
  for i, value in ipairs({3840,1920,960,480,240,120,60,30,2560,1280,640,320,160,80,40}) do
    if ticks == value then return GRID_NAMES[i] end
  end
  return ticks .. "t"
end
local function bars(s)
  return (s.pattern_length or 3840) / (960 * 4 / math.max(1, s.denominator or 4) * math.max(1, s.numerator or 4))
end
local function control_buttons(s)
  local c = s.control or {}
  return {
    {label=held[49] and "UNDO" or "MASTER", selected=not held[49] and c.level == 1},
    {label=held[49] and "REDO" or "GROUP", selected=not held[49] and c.level == 2},
    {label="SOUND", selected=c.level == 3}, {label="PLUG-IN", selected=c.plugins},
    {label="PAGE -", enabled=(c.page or 1) > 1}, {label="PAGE +", enabled=(c.page or 1) < #(c.pages or {})},
    {label="SLOT -", enabled=(c.slot_index or 1) > 1}, {label="SLOT +", enabled=c.slot_index ~= nil},
  }
end
local function split_buttons(buttons)
  button_bar(0, {buttons[1], buttons[2], buttons[3], buttons[4]})
  button_bar(1, {buttons[5], buttons[6], buttons[7], buttons[8]})
end
local function pad_cell(screen, key, x, y, w, h, number, label, active, selected, value, muted, fill)
  region(screen, key, x, y, w, h, function()
    rect(screen, x, y, w - 1, h - 1, active and 0 or 2)
    local number_width = label and math.max(12, #ascii(number) * 6 - 1) or 27
    text(screen, x + 3, y + 1, number, number_width)
    if label then text(screen, x + number_width + 7, y + 1, label, w - number_width - 10) end
    if value then rect(screen, x + 30, y + 2, math.floor(clamp(value, 0, 1) * (w - 35)), h - 5, 1) end
    if muted then rect(screen, x + 2, y + h - 3, w - 5, 1, 1) end
    local filled = fill == nil and active or fill
    if filled then rect(screen, x + 1, y + 1, w - 3, h - 3, 4) end
    if selected then
      rect(screen, x + w - 7, y + 2, 2, h - 5, filled and 5 or 1)
    end
  end)
end
local function draw_normal(s, group, sound)
  local c = s.control or {}
  local buttons = control_buttons(s)
  local modifier = held[33] and "MUTE" or held[31] and "SOLO" or held[30] and "SELECT" or held[29] and "DUPLICATE" or held[2] and "ERASE" or nil
  begin_layout(modifier and "sounds:" .. modifier or "control")
  split_buttons(buttons)
  local right = wheel == "tempo" and string.format("%.1f BPM", s.tempo or 120)
    or wheel == "swing" and string.format("SW %.0f%%", s.swing or 0)
    or wheel == "volume" and (s.volume_db and string.format("%.1fdB", s.volume_db) or string.format("VOL %.0f%%", (s.volume or 0) * 100))
    or s.recording and "REC" or s.playing and "PLAY" or "STOP"
  info(0, group_label(s) .. " " .. (group and group.name or "") .. "/" .. (sound and sound.name or "No sound"), right)
  local page, pages = c.page or 1, c.pages or {}
  info(1, (c.slot or (c.plugins and "+ Add plug-in" or "Channel")) .. " / " .. (pages[page] or ""), page .. "/" .. math.max(1, #pages))
  if modifier then
    region(0, "held", 0, 21, 255, 43, function()
      text(0, 3, 23, modifier, 249, 2)
      local help = modifier == "DUPLICATE" and (duplicate_from and ("PAD " .. duplicate_from .. " > DESTINATION") or "SELECT SOURCE PAD")
        or modifier == "ERASE" and (held[49] and "PAD CLEARS SOUND" or "PAD ERASES PATTERN NOTES")
        or "PRESS A SOUND OR GROUP"
      text(0, 3, 44, help, 249)
      text(0, 3, 55, group and group.name or "No group", 249)
    end)
    for n = 1, 16 do
      local item = group and group.sounds and group.sounds[n]
      local x, y = ((n - 1) % 4) * 64, 22 + (3 - (n - 1) // 4) * 10
      pad_cell(1, "sound" .. n, x, y, 63, 10, string.format("%02d", n), item and item.name or "", item and item.used,
        n == (duplicate_from or s.sound), nil, item and item.muted, n == (duplicate_from or s.sound))
    end
  else
    for screen = 0, 1 do
      for column = 1, 4 do parameter(screen, column, c.knobs and c.knobs[screen * 4 + column], 21, 43) end
    end
  end
end
local function pattern_overview(s)
  region(0, "pattern-overview", 0, 21, 255, 30, function()
    local start, length = s.pattern_start or 0, math.max(1, s.pattern_length or 3840)
    local function px(at) return 1 + math.floor(clamp((at - start) / length, 0, 1) * 251) end
    for i = 0, 4 do rect(0, 1 + math.floor(i * 251 / 4), 22, 1, 27, 2) end
    local lane = 23 + math.floor(((s.sound or 1) - 1) * 24 / 16)
    rect(0, 1, lane, 252, 1, 3)
    for _, note in ipairs(s.notes or {}) do
      local from, to = math.max(start, note.position or 0), math.min(start + length, (note.position or 0) + math.max(1, note.length or 120))
      if to > from then
        local y = 23 + math.floor(((note.sound or 1) - 1) * 24 / 16)
        rect(0, px(from), y, math.max(1, px(to) - px(from)), note.sound == s.sound and 2 or 1, 1)
      end
    end
    local first, _, step_length = step_grid(s)
    local from = first + step_page * 16 * step_length
    local a, b = px(from), px(from + 16 * step_length)
    rect(0, a, 21, math.max(2, b - a + 1), 30, 0)
    if s.pattern_position then rect(0, px(s.pattern_position), 22, 1, 28, 4) end
  end)
end
local function draw_step(s, group, sound)
  begin_layout("step")
  local grid = s.grid or {}
  local _, _, _, last_page = step_grid(s)
  split_buttons({{label="PATTERNS"}, {label="SNAP", selected=grid.step_snap},
    {label="GRID -", enabled=(grid.step or 4) > 0}, {label="GRID +", enabled=(grid.step or 4) < #GRID_NAMES - 1},
    {label="FOLLOW", selected=following(s)}, {label="DOUBLE"}, {label="PAGE -", enabled=step_page > 0}, {label="PAGE +", enabled=step_page < last_page}})
  info(0, group_label(s) .. " " .. (group and group.name or "") .. "/" .. (sound and sound.name or "No sound"), "STEP")
  local pattern = group and group.patterns and group.patterns[s.pattern or 1]
  info(1, pattern and pattern.name or "No pattern", string.format("%03d-%03d", step_page * 16 + 1, (step_page + 1) * 16))
  pattern_overview(s)
  region(0, "step-footer", 0, 51, 255, 13, function()
    text(0, 2, 52, string.format("%.2g BAR", bars(s)), 66)
    text(0, 76, 52, grid_label(s), 70)
    text(0, 156, 52, string.format("PAGE %d/%d", step_page + 1, last_page + 1), 98)
    rect(0, 2, 61, 251, 2, 0)
    local count = last_page + 1
    rect(0, 3 + math.floor(step_page * 249 / count), 61, math.max(1, math.ceil(249 / count)), 2, 1)
  end)
  for n = 1, 16 do
    local at, length, inside = step_at(s, n)
    local active, velocity = false, 0
    for _, note in ipairs(s.notes or {}) do
      if note.sound == s.sound and note.position >= at and note.position < at + length then
        active, velocity = true, math.max(velocity, note.velocity or 0)
      end
    end
    local playing = inside and s.pattern_position and s.pattern_position >= at and s.pattern_position < at + length
    local x, y = ((n - 1) % 4) * 64, 22 + (3 - (n - 1) // 4) * 10
    pad_cell(1, "step" .. n, x, y, 63, 10, inside and string.format("%03d", step_page * 16 + n) or "", nil,
      inside and active, playing, active and velocity / 127 or nil)
  end
end
local function draw_picker(s, group, sound)
  local patterns = view_mode == "pattern"
  local bank_index = patterns and pattern_bank or scene_bank
  begin_layout(patterns and "patterns" or "scenes")
  split_buttons({{label="PIN", selected=pinned == view_mode}, {}, {label=patterns and "DOUBLE" or ""},
    {label=patterns and "DUPLICATE" or ""}, {label=patterns and "NEW" or ""}, {label=patterns and "DELETE" or ""},
    {label="BANK -", enabled=bank_index > 0}, {label="BANK +"}})
  info(0, group_label(s) .. " " .. (group and group.name or "No group"), patterns and "PATTERN" or "SCENE")
  local index = patterns and s.pattern or s.scene
  local item = patterns and group and group.patterns and group.patterns[index or 1] or not patterns and s.scenes and s.scenes[index or 1]
  info(1, item and item.name or "Empty", "BANK " .. (bank_index + 1))
  region(0, "picker-detail", 0, 21, 255, 43, function()
    text(0, 3, 23, string.format("%02d", item and (item.slot or index) or 0), 38, 2)
    text(0, 46, 24, item and item.name or "Empty slot", 205)
    text(0, 3, 43, patterns and string.format("%.2g BARS  %s", bars(s), grid_label(s)) or ("SOUNDS  " .. (sound and sound.name or "-")), 249)
    if patterns then text(0, 3, 55, "K1 LENGTH   EMPTY PAD CREATES", 249)
    else
      local count = 0
      for _, p in pairs(item and item.patterns or {}) do if p then count = count + 1 end end
      text(0, 3, 55, count .. " GROUP PATTERNS", 249)
    end
  end)
  for n = 1, 16 do
    local place = bank_index * 16 + n
    local found, entry
    if patterns then found, entry = pattern_at(group, place) else found, entry = place, s.scenes and s.scenes[place] end
    local x, y = ((n - 1) % 4) * 64, 22 + (3 - (n - 1) // 4) * 10
    pad_cell(1, "pick" .. n, x, y, 63, 10, string.format("%02d", place), entry and entry.name or "", entry ~= nil, entry and found == index, nil, nil, entry and found == index)
  end
end
local function draw_browser(s)
  local b, c = s.browser or {}, s.control or {}
  local results = b.results or {}
  begin_layout(#results > 0 and "browse-list" or "browse-empty")
  local kind = b.kind or browser_kind
  split_buttons({{label="TYPE -", enabled=kind > 1}, {label="TYPE +", enabled=kind < 7},
    {label="USER", selected=b.user}, {label=b.user and "FACTORY" or "USER"},
    {label="PREVIOUS", enabled=(b.focus or 1) > 1}, {label="NEXT", enabled=(b.focus or 1) < #results},
    {label=held[49] and "SET FAV" or "FAVORITES", selected=b.favorites}, {label=held[49] and "PREHEAR" or "LOAD", enabled=#results > 0}})
  local kinds = {"PROJECTS","GROUPS","SOUNDS","INSTRUMENTS","EFFECTS","LOOPS","ONE-SHOTS"}
  info(0, kinds[kind] or "BROWSER", b.busy and "SEARCH" or b.user and "USER" or "FACTORY")
  info(1, c.slot or "Results", string.format("%d/%d", #results > 0 and (b.focus or 1) or 0, #results))
  local product = b.product and b.products and b.products[b.product] or "All products"
  local filters = {{"K2 PRODUCT", product}}
  for n, name in ipairs({"K5 TYPE", "K6 SUBTYPE", "K7 CHARACTER"}) do
    local a = b.attributes and b.attributes[n] or {}
    filters[n + 1] = {name, a.chosen and a.values and a.values[a.chosen] or "All"}
  end
  for n = 1, 4 do
    local y = 22 + (n - 1) * 10
    region(0, "filter" .. n, 0, y, 255, 10, function()
      text(0, 2, y + 1, filters[n][1], 78)
      text(0, 86, y + 1, filters[n][2], 165)
      rect(0, 80, y + 1, 1, 7, 1)
    end)
  end
  if #results == 0 then
    region(1, "results-empty", 0, 21, 255, 43, function()
      centered(1, 0, 29, 255, b.busy and "SEARCHING..." or b.results == nil and "BROWSER DATA UNAVAILABLE" or "NO RESULTS")
      centered(1, 0, 45, 255, b.results == nil and "OPEN THE DECKARD WINDOW" or "CHANGE TYPE OR FILTERS")
    end)
    return
  end
  local focus = clamp(b.focus or 1, 1, #results)
  local start = clamp(focus - 1, 1, math.max(1, #results - 3))
  -- Empty/list transitions reset only the right-hand list, so its old message
  -- cannot survive behind a shorter result set.
  for row = 1, 4 do
    local index, y = start + row - 1, 22 + (row - 1) * 10
    local entry = results[index]
    region(1, "result" .. row, 0, y, 255, 10, function()
      if not entry then return end
      if entry.favorite then
        rect(1, 3, y + 3, 5, 1, 1); rect(1, 5, y + 1, 1, 5, 1)
      end
      text(1, 12, y + 1, entry.name, 237)
      if index == focus then rect(1, 1, y, 249, 9, 4) end
      rect(1, 252, y, 2, 9, index == focus and 1 or 2)
    end)
  end
end
-- A column combines actual min/max peaks with its markers. Moving the playhead
-- changes only its old and new columns. All 480 source peak columns contribute.
local function draw_wave(screen, key, sample, which, peaks, y, h, slices)
  if #peaks < 2 then
    region(screen, key .. "empty", 0, y, 255, h, function()
      centered(screen, 0, y + math.max(1, (h - 8) // 2), 255, "WAVEFORM UNAVAILABLE")
    end)
    return
  end
  region(screen, key .. "frame", 0, y, 255, h, function() rect(screen, 0, y, 255, h, 0) end)
  local a, b = 0, sample.frames
  if which == "detail" and sample.view then a, b = sample.view[1], sample.view[2] end
  local span, count, width = math.max(1, b - a), #peaks // 2, 253
  local play, selection, looped = sample.play or {0, sample.frames}, sample.selection, sample.looped
  local function column(frame) return math.floor((frame - a) / span * width) end
  local start, finish = column(play[1]), column(play[2])
  local cursor = sample.playhead and column(sample.playhead)
  local select_start, select_end = selection and column(selection[1]), selection and column(selection[2])
  local loop_start, loop_end = looped and column(looped[1]), looped and column(looped[2])
  local view_start, view_end = sample.view and column(sample.view[1]), sample.view and column(sample.view[2])
  local marks = {}
  if slices then
    for _, at in ipairs(sample.slices or {}) do marks[column(at)] = true end
    selection = nil
    local list = sample.slices or {}
    if list[sample_slice] then select_start, select_end = column(list[sample_slice]), column(list[sample_slice + 1] or play[2]) end
  end
  local columns = wave_columns[screen]
  if not columns then columns = {}; wave_columns[screen] = columns end
  local packet, last_column, packet_columns
  local function flush()
    if not packet then return end
    packet[#packet + 1] = 0xf7
    deckard.midi_send("controls", packet)
    packet = nil
  end
  for c = 0, width - 1 do
    local from = math.floor(c * count / width)
    local to = math.max(from, math.floor((c + 1) * count / width) - 1)
    local low, high = 1, -1
    for i = from, math.min(count - 1, to) do
      low, high = math.min(low, peaks[2 * i + 1] or 0), math.max(high, peaks[2 * i + 2] or 0)
    end
    local top = clamp(math.floor((1 - clamp(high, -1, 1)) * (h - 3) / 2), 0, h - 3)
    local bottom = clamp(math.ceil((1 - clamp(low, -1, 1)) * (h - 3) / 2), top, h - 3)
    local flags = 0
    if (c >= start and c < finish) or c % 2 == 0 then flags = flags | 1 end
    if select_start and c >= select_start and c < select_end then flags = flags | 2 end
    if loop_start and c >= loop_start and c <= loop_end then
      flags = flags | 4
      if c == loop_start or c == loop_end then flags = flags | 8 end
    end
    if marks[c] then flags = flags | 16 end
    if c == start or c == finish then flags = flags | 32 end
    if which == "overview" and sample.view and (view_start > 0 or view_end < width)
        and c >= view_start and c <= view_end then
      flags = flags | 64
      if c == view_start or c == view_end then flags = flags | 128 end
    end
    if c == cursor then flags = flags | 256 end
    -- Quantized geometry, not Lua tables or command strings, is the cache key.
    -- A packet holds at most thirteen adjacent changed columns (63 MIDI bytes).
    local signature = top | (bottom << 6) | (flags << 12)
    if columns[c] ~= signature then
      columns[c] = signature
      if packet and (c ~= last_column + 1 or packet_columns == 13) then flush() end
      if not packet then
        local x = c + 1
        packet = {0xf0,0x7d,0x44,0x47,3,screen,x % 128,x // 128,y + 1,h - 2}
        packet_columns = 0
      end
      packet[#packet + 1], packet[#packet + 2] = top, bottom
      packet[#packet + 1], packet[#packet + 2] = flags % 128, flags // 128
      packet_columns, last_column = packet_columns + 1, c
    end
  end
  flush()
end
local function draw_zone_map(sample)
  region(0, "zone-map", 0, 21, 255, 25, function()
    for key = 0, 120, 12 do rect(0, 1 + math.floor(key / 128 * 251), 22, 1, 23, 2) end
    rect(0, 1, 22, 252, 23, 0)
    for index, zone in ipairs(sample.zones or {}) do
      local x = 2 + math.floor(zone[1] / 128 * 249)
      local w = math.max(2, math.floor((zone[2] + 1 - zone[1]) / 128 * 249))
      local y = 23 + math.floor((127 - zone[4]) / 128 * 20)
      local h = math.max(2, math.floor((zone[4] + 1 - zone[3]) / 128 * 20))
      local selected = index == (sample.focus_zone or 1)
      rect(0, x, y, w, h, selected and 0 or 2)
      if selected then text(0, x + 2, y + 1, index, math.max(0, w - 4)) end
    end
  end)
end
local function draw_sampling(s, group, sound)
  local v = sample_state(s)
  local tab, sample = v.tab or 1, v.sample
  local has = sample_ok(v)
  local overview, detail = {}, {}
  if has and deckard.peaks then
    detail = deckard.peaks("detail") or {}
    if tab == 2 or tab == 3 then overview = deckard.peaks("overview") or {} end
  end
  local detail_available, overview_available = #detail >= 2, #overview >= 2
  begin_layout("sampling:" .. tab .. ":" .. tostring(has) .. ":" .. tostring(detail_available) .. ":" .. tostring(overview_available) .. ":" .. tostring(v.stems and v.stems.focused or false))
  local tabs = {}
  for n = 1, 4 do tabs[n] = {label=SAMPLE_TABS[n], selected=n == tab} end
  button_bar(0, tabs)
  button_bar(1, sample_buttons(v))
  local page, number, count = sample_page(v)
  info(0, group_label(s) .. " S" .. string.format("%02d", s.sound or 1) .. " " .. (page.name or SAMPLE_TABS[tab]), number .. "/" .. math.max(1, count))
  local sample_info = has and seconds(sample.frames, sample.rate) or "EMPTY"
  if tab == 1 then sample_info = "TAKE " .. (v.take or 0) .. "/" .. (v.takes or 0) end
  info(1, sample and sample.name or (sound and sound.name or "No sound"), sample_info)
  if v.stems and v.stems.focused then
    region(0, "stems", 0, 21, 255, 25, function()
      text(0, 3, 24, "STEM SEPARATION", 249)
      rect(0, 3, 36, 248, 7, 0)
      rect(0, 5, 38, math.floor(clamp(v.stems.progress or 0, 0, 1) * 244), 3, 1)
    end)
    region(1, "stems", 0, 21, 255, 32, function()
      centered(1, 0, 25, 255, string.format("%.0f%%", (v.stems.progress or 0) * 100), 2)
      centered(1, 0, 44, 255, "F8 CANCEL")
    end)
  else
    if tab == 1 then
      local record = v.record or {}
      meter(0, "meterL", 22, record.levels and record.levels[1], record.mode == 0 and record.threshold or nil)
      meter(0, "meterR", 34, record.levels and record.levels[2], record.mode == 0 and record.threshold or nil)
    elseif has and tab == 4 then draw_zone_map(sample)
    elseif has then draw_wave(0, "overview", sample, "overview", overview, 22, 24, tab == 3)
    else
      region(0, "no-sample", 0, 21, 255, 25, function()
        centered(0, 0, 23, 255, v.no_sample and v.no_sample.title or "NO SAMPLE")
        centered(0, 0, 35, 255, v.no_sample and v.no_sample.detail or "RECORD OR LOAD A SAMPLE")
      end)
    end
    if has then draw_wave(1, "detail", sample, "detail", detail, 22, 31, tab == 3)
    else
      region(1, "no-wave", 0, 21, 255, 32, function()
        centered(1, 0, 25, 255, tab == 1 and (sample_busy(v) and (v.record.waiting and "WAITING FOR INPUT" or "RECORDING") or "READY TO RECORD") or "NO SAMPLE")
        centered(1, 0, 40, 255, tab == 1 and ("TAKES " .. (v.takes or 0)) or "BROWSE LOADS A SAMPLE")
      end)
    end
  end
  for column = 1, 4 do parameter(0, column, page.knobs and page.knobs[column], 46, 18) end
  region(1, "sample-footer", 0, 54, 255, 10, function()
    if tab == 1 then
      local takes = v.takes or 0
      for n = 1, math.min(16, math.max(0, takes - sample_bank * 16)) do
        local index, x = sample_bank * 16 + n, (n - 1) * 16
        text(1, x + 2, 55, n, 13)
        rect(1, x, 54, 15, 10, 0)
        if index == v.take then rect(1, x + 1, 55, 13, 8, 4) end
      end
    elseif tab == 2 then
      text(1, 2, 55, v.stems and v.stems.focused and "STEMS RUNNING" or SAMPLE_FUNCTIONS[sample_function][1], 126)
      if sample and sample.view then text(1, 132, 55, "ZOOM " .. string.format("%.1fx", sample.frames / math.max(1, sample.view[2] - sample.view[1])), 121) end
    elseif tab == 3 then
      text(1, 2, 55, "SLICE " .. sample_slice .. "/" .. (sample and #(sample.slices or {}) or 0), 126)
      text(1, 132, 55, "BANK " .. ((sample_slice - 1) // 16 + 1), 121)
    else
      local zone = sample and (sample.zones or {})[sample.focus_zone or 1]
      text(1, 2, 55, "ZONE " .. (sample and sample.focus_zone or 0) .. "/" .. (sample and #(sample.zones or {}) or 0), 100)
      if zone then text(1, 108, 55, string.format("KEY %d-%d", zone[1], zone[2]), 145) end
    end
  end)
end
local function draw(s, group, sound)
  if sampling then draw_sampling(s, group, sound)
  elseif browsing then draw_browser(s)
  elseif view_mode == "pattern" or view_mode == "scene" then draw_picker(s, group, sound)
  elseif step_mode then draw_step(s, group, sound)
  else draw_normal(s, group, sound) end
end

function M.open()
  sent, values, held, regions = {}, {}, {}, {}
  layout, wave_columns = nil, {}
  sample_pages, sample_turns, sample_asked = {1,1,1,1}, {}, {}
  sample_function, sample_slice, sample_bank, sample_identity = 1, 1, 0, nil
  asked_follow, sample_take = nil, nil
  browsing, sampling, wheel, user_content = false, false, nil, false
  step_mode, step_page, duplicate_from, browser_kind = false, 0, nil, 1
  view_mode, pinned, mode_before, mode_down, mode_started, pin_touched = nil, nil, nil, nil, 0, false
  pattern_bank, scene_bank, knob_acc = 0, 0, {}
  set_step(false)
  deckard.message("Maschine MK2 conectada")
end

function M.midi(role, bytes)
  if role ~= "controls" then return end
  local status = (bytes[1] or 0) & 0xf0
  local key, value = bytes[2] or 0, bytes[3] or 0
  local s = deckard.state()
  if status == 0x90 and value > 0 and key >= 36 and key <= 51 then
    if sampling and not (held[2] or held[29] or held[30] or held[31] or held[33]) and sample_pad(key - 35, s) then return end
    pad_action(key - 35, s, value)
    return
  end
  if status ~= 0xb0 then return end
  if key == 15 then
    local delta = relative(key, value)
    if sampling and wheel == nil then
      local amount = steps(delta, 8)
      if amount ~= 0 then sample_navigate(amount, s) end
    else wheel_turn(delta, s) end
    return
  elseif key >= 16 and key <= 23 then
    local delta = relative(key, value)
    if delta ~= 0 then
      if sampling then sample_knob(key - 15, delta, s)
      elseif (view_mode == "pattern" or (step_mode and not view_mode)) and key == 16 then pattern_length_turn(s, delta, key)
      elseif step_mode and not view_mode and key == 17 then change_grid(s, steps(delta, 4))
      elseif browsing then browse_knob(key - 15, delta, s)
      else deckard.turn_knob(key - 15, delta / 127, held[49] or false) end
    end
    return
  end
  held[key] = value > 0
  if key == 25 then
    mode_button("scene", value > 0)
    return
  elseif key == 26 then
    if value > 0 then
      if held[49] then mode_button("pattern", true)
      else set_step(not (step_mode and not view_mode)) end
    elseif mode_down == "pattern" then mode_button("pattern", false) end
    return
  end
  if key == 29 and value == 0 then duplicate_from = nil end
  if key == 10 then
    deckard.note_repeat("on", value > 0)
    return
  end
  if value == 0 then return end
  if sampling then
    if key >= 39 and key <= 46 then sample_action(key - 38, s); return
    elseif key == 47 or key == 48 then sample_page_move(sample_state(s), key == 47 and 1 or -1); return
    elseif key == 13 or key == 14 then sample_navigate(key == 13 and -1 or 1, s); return
    elseif key == 36 then
      if held[49] and sample_ok(sample_state(s)) then deckard.sampling_call("view", 0, s.sampling.sample.frames)
      else sample_prehear(sample_state(s)) end
      return
    end
  end
  if key >= 80 and key <= 87 then
    local group = bank(s) + key - 79
    if s.groups[group] then
      if held[33] then deckard.mute_group(group)
      elseif held[31] then deckard.solo_group(group)
      else deckard.focus_group(group) end
    elseif group == #s.groups + 1 and not held[33] and not held[31] then
      deckard.add_group()
    end
  elseif key == 1 then
    if s.playing then deckard.stop() else deckard.play() end
  elseif key == 3 then
    deckard.record(not s.recording, held[49] or false)
  elseif key == 4 then
    local grid = s.grid or {}
    deckard.grid("step_snap", not grid.step_snap)
  elseif key == 5 or key == 6 then
    local delta = key == 5 and -1 or 1
    if step_mode and not view_mode then step_page_move(s, delta)
    else
      local group = s.groups[s.group]
      local selected = (s.pattern or 1) + delta
      if group and group.patterns and group.patterns[selected] then deckard.pick_pattern(selected) end
    end
  elseif key == 7 then
    if held[49] then deckard.set_loop(not s.looping) else deckard.restart() end
  elseif key == 8 then
    browsing = not browsing
    if browsing then
      if sampling then sampling = false; deckard.sampling(false) end
      view_mode, pinned, step_mode = nil, nil, false
    end
    sync_mode(s)
    deckard.library(browsing)
  elseif key == 9 then
    sampling = not sampling
    if sampling then
      if browsing then browsing = false; deckard.library(false) end
      view_mode, pinned, step_mode = nil, nil, false
    end
    deckard.sampling(sampling)
    sync_mode(s)
  elseif key == 11 then
    if browsing then browsing = false; deckard.library(false) end
    if sampling then sampling = false; deckard.sampling(false) end
    set_step(false)
    deckard.show_plugins(false)
    deckard.level(3)
  elseif key == 13 then
    if browsing then deckard.browse_focus(-1) else deckard.slot(-1) end
  elseif key == 14 then
    if browsing then deckard.browse_focus(1) else deckard.slot(1) end
  elseif key == 24 then
    deckard.level(1)
  elseif key == 27 then
    if browsing then browsing = false; deckard.library(false) end
    if sampling then sampling = false; deckard.sampling(false) end
    set_step(false)
  elseif key == 28 then
    deckard.show_plugins(not (s.control and s.control.plugins))
  elseif key == 32 then
    set_follow(not following(s), s)
  elseif key == 34 then
    deckard.song_view(not s.song_view)
  elseif key == 36 then
    if browsing then deckard.browse_load() else deckard.slot_edit("edit") end
  elseif key == 37 then
    deckard.auto_write(not s.auto_write)
  elseif key == 47 then
    if view_mode == "pattern" then pattern_bank = pattern_bank + 1
    elseif view_mode == "scene" then scene_bank = scene_bank + 1
    elseif step_mode then step_page_move(s, 1)
    else deckard.next_page() end
  elseif key == 48 then
    if view_mode == "pattern" then pattern_bank = math.max(0, pattern_bank - 1)
    elseif view_mode == "scene" then scene_bank = math.max(0, scene_bank - 1)
    elseif step_mode then step_page_move(s, -1)
    else deckard.previous_page() end
  elseif key == 35 or key == 50 or key == 51 then
    local choice = ({ [35] = "tempo", [50] = "swing", [51] = "volume" })[key]
    if wheel == choice then wheel = nil else wheel = choice end
  elseif key >= 39 and key <= 46 then
    local n = key - 38
    if view_mode == "pattern" or view_mode == "scene" then
      if n == 1 then
        if pinned == view_mode then pinned = nil else pinned = view_mode end
        pin_touched = true
        if not pinned and not (held[25] or held[26]) then view_mode = mode_before; sync_mode() end
      elseif view_mode == "pattern" and n >= 3 and n <= 6 then
        deckard.pattern_edit(({ [3]="double", [4]="duplicate", [5]="create", [6]="delete" })[n])
      elseif n == 7 then
        if view_mode == "pattern" then pattern_bank = math.max(0, pattern_bank - 1) else scene_bank = math.max(0, scene_bank - 1) end
      elseif n == 8 then
        if view_mode == "pattern" then pattern_bank = pattern_bank + 1 else scene_bank = scene_bank + 1 end
      end
    elseif step_mode then
      if n == 1 then mode_before, view_mode, pinned = nil, "pattern", "pattern"; sync_mode()
      elseif n == 2 then deckard.grid("step_snap", not (s.grid or {}).step_snap)
      elseif n == 3 or n == 4 then change_grid(s, n == 3 and -1 or 1)
      elseif n == 5 then set_follow(not following(s), s)
      elseif n == 6 then deckard.pattern_edit("double")
      elseif n == 7 then step_page_move(s, -1)
      elseif n == 8 then step_page_move(s, 1) end
    elseif browsing then
      if n == 1 or n == 2 then
        browser_kind = math.max(1, math.min(7, ((s.browser or {}).kind or browser_kind) + (n == 1 and -1 or 1)))
        deckard.browse_kind(browser_kind)
      elseif n == 3 then user_content = true; deckard.browse_user(true)
      elseif n == 4 then user_content = not ((s.browser or {}).user == true); deckard.browse_user(user_content)
      elseif n == 5 then deckard.browse_focus(-1)
      elseif n == 6 then deckard.browse_focus(1)
      elseif n == 7 then
        if held[49] then deckard.browse_set_favorite() else deckard.browse_favorites() end
      elseif n == 8 then
        if held[49] then deckard.browse_prehear() else deckard.browse_load() end
      end
    elseif held[49] and n == 1 then deckard.edit("undo")
    elseif held[49] and n == 2 then deckard.edit("redo")
    elseif n <= 3 then deckard.level(n)
    elseif n == 4 then deckard.show_plugins(not (s.control and s.control.plugins))
    elseif n == 5 then deckard.previous_page()
    elseif n == 6 then deckard.next_page()
    elseif n == 7 then deckard.slot(-1)
    elseif n == 8 then deckard.slot(1) end
  end
end

function M.tick()
  local s = deckard.state()
  if sampling then sample_sync(s) end
  if step_mode then
    if following(s) and s.playing and s.pattern_position then
      local first, _, length = step_grid(s)
      step_page = math.max(0, math.floor((s.pattern_position - first) / (16 * length)))
    end
    step_page_move(s, 0)
  end
  sync_mode(s)
  local groups = s.groups or {}
  for i = 1, 8 do
    local index = bank(s) + i
    local group = groups[index]
    if group then
      local level = group.muted and .12 or ((s.group == index or (s.playing and group.playing)) and .8 or .25)
      rgb(i - 1, group.color, level)
      local indicator = 0
      if not group.muted then
        if group.playing then indicator = 1
        elseif group.playing_progress ~= nil then indicator = .45
        elseif group.scene_pattern then indicator = .12 end
      end
      rgb(64 + i, group.color, indicator)
    else
      light(i - 1, 0, 0, 0)
      light(64 + i, 0, 0, 0)
    end
  end
  local control = s.control or {}
  local page, pages = control.page or 1, control.pages or {}
  local flags = {
    [8] = s.playing and 100 or 0, [9] = s.recording and 100 or 0,
    [10] = held[49] and 100 or 0, [11] = page > 1 and 55 or 0,
    [12] = page < #pages and 55 or 0, [13] = 20,
    [14] = 25, [15] = following(s) and 100 or 0, [16] = browsing and 100 or 25,
    [17] = sampling and 100 or 25, [18] = 0, [19] = s.auto_write and 100 or 25,
    [28] = view_mode == "scene" and 100 or 25, [29] = (step_mode or view_mode == "pattern") and 100 or 25,
    [30] = (step_mode or view_mode or sampling) and 25 or 100, [31] = 25, [32] = held[29] and 100 or 25, [33] = held[30] and 100 or 25,
    [34] = held[31] and 100 or 25, [35] = held[33] and 100 or 25,
    [36] = wheel == "volume" and 100 or 25, [37] = wheel == "swing" and 100 or 25,
    [38] = wheel == "tempo" and 100 or 25, [39] = 25, [40] = 25,
    [41] = 25, [42] = held[10] and 100 or 25, [43] = s.grid and s.grid.step_snap and 100 or 25,
    [44] = 25, [45] = 25, [46] = held[2] and 100 or 25,
  }
  for i = 20, 27 do flags[i] = 25 end
  if view_mode then flags[20] = pinned == view_mode and 100 or 25 end
  if view_mode == "pattern" then flags[21] = 0 end
  if sampling then
    local v = sample_state(s)
    local _, index, count = sample_page(v)
    flags[11], flags[12] = index > 1 and 55 or 0, index < count and 55 or 0
    for n = 1, 4 do flags[19 + n] = (v.tab or 1) == n and 100 or 25 end
    for n, spec in ipairs(sample_buttons(v)) do
      if spec.label and spec.enabled ~= false then flags[23 + n] = spec.selected and 100 or 25
      else flags[23 + n] = 0 end
    end
    if v.tab == 1 and v.record and v.record.waiting then flags[24] = (deckard.now() * 4) % 2 < 1 and 100 or 25 end
  elseif browsing then
    local b = s.browser or {}
    flags[22], flags[23] = b.user and 100 or 25, b.user and 25 or 100
    flags[26] = b.favorites and 100 or 25
    flags[24] = (b.focus or 1) > 1 and 25 or 0
    flags[25] = (b.focus or 1) < #(b.results or {}) and 25 or 0
    flags[27] = #(b.results or {}) > 0 and 25 or 0
  elseif step_mode and not view_mode then
    local _, _, _, last_page = step_grid(s)
    flags[20], flags[21], flags[24] = 25, (s.grid or {}).step_snap and 100 or 25, following(s) and 100 or 25
    flags[26], flags[27] = step_page > 0 and 25 or 0, step_page < last_page and 25 or 0
  end
  for id = 8, 46 do local v = flags[id] or 0; light(id, v, v, v) end
  local group = groups[s.group or 1]
  for i = 1, 16 do
    local item = group and group.sounds and group.sounds[i]
    local pattern_index
    if view_mode == "scene" then item = s.scenes and s.scenes[scene_bank * 16 + i]
    elseif view_mode == "pattern" then pattern_index, item = pattern_at(group, pattern_bank * 16 + i) end
    local custom_sample = sampling and not (held[29] or held[2] or held[30] or held[31] or held[33])
    local v = custom_sample and sample_state(s) or nil
    if v and (v.tab == 1 or v.tab == 3) then
      local index = (v.tab == 1 and sample_bank or (sample_slice - 1) // 16) * 16 + i
      local count = v.tab == 1 and (v.takes or 0) or v.sample and #(v.sample.slices or {}) or 0
      local selected = index == (v.tab == 1 and v.take or sample_slice)
      local current = group and group.sounds and group.sounds[s.sound or 1]
      if index <= count then rgb(47 + i, current and current.color, selected and 1 or .3)
      else light(47 + i, 0, 0, 0) end
    elseif step_mode and not (view_mode or held[29] or held[2] or held[30] or held[31] or held[33]) then
      local at, length, inside = step_at(s, i)
      local active = inside and step_active(s, at, length)
      local playing = inside and s.pattern_position and s.pattern_position >= at and s.pattern_position < at + length
      local current = group and group.sounds and group.sounds[s.sound or 1]
      if inside then rgb(47 + i, current and current.color, active and 1 or playing and .45 or .08)
      else light(47 + i, 0, 0, 0) end
    elseif item and (item.used ~= false or i == s.sound or item.playing) then
      local selected = (view_mode == "scene" and scene_bank * 16 + i == s.scene)
        or (view_mode == "pattern" and pattern_index == s.pattern)
        or (not view_mode and (i == s.sound or item.playing))
      local level = item.muted and .1 or (selected and 1 or .3)
      rgb(47 + i, item.color, level)
    else light(47 + i, 0, 0, 0) end
  end
  local sound = group and group.sounds and group.sounds[s.sound or 1]
  draw(s, group, sound)
end

function M.close()
  sampling, browsing = false, false
  set_step(false)
  for id = 0, 46 do light(id, 0, 0, 0) end
  for id = 48, 63 do light(id, 0, 0, 0) end
  for id = 65, 72 do light(id, 0, 0, 0) end
  for screen = 0, 1 do deckard.midi_send("controls", {0xf0,0x7d,0x44,0x47,0,screen,0xf7}) end
end

return M
