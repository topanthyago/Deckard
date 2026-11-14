local root, capture_path = arg[1], arg[2]
local manifest = dofile(root .. "/device.lua")
assert(manifest.drums[1] == "Pads MIDI" and manifest.ports.controls[1] == "Controls MIDI")
assert(manifest.exclusive[1] == "controls")
local capture = capture_path and assert(io.open(capture_path, "w"))
local calls, sent, now = {}, {}, 0
local state = {
  groups={{name="João Drums", color={255,0,0}, patterns={
    {slot=1,name="Main Beat",color={255,0,0},bars=2}, {slot=3,name="Break",color={0,0,255},bars=2}},
    sounds={{name="Kick",color={255,0,0},used=true}, {name="Snare",color={0,0,255},used=true},
      {name="Closed HH",color={255,180,0},used=true}, {name="Open HH",color={255,100,0},used=true}}},
    {name="Bass",color={0,0,255},sounds={}}},
  scenes={{name="Intro",color={0,255,0},patterns={1,false}}, {name="Verse",color={0,200,255},patterns={2,1}}},
  group=1,sound=1,scene=1,pattern=1,tempo=123.5,swing=13,volume=.75,volume_db=0,
  pattern_start=0,pattern_length=7680,grid={step=4,step_ticks=240,step_snap=true},
  notes={{sound=1,position=0,length=120,velocity=127},{sound=1,position=480,length=120,velocity=96},
    {sound=2,position=240,length=120,velocity=100},{sound=3,position=720,length=100,velocity=80}},
  control={level=3,page=1,pages={"Voice","Envelope"},slot="Sampler",slots=2,slot_index=1,knobs={
    {name="Tune",text="0.00st",value=.5},{name="Start",text="0.000s",value=0},
    {name="Decay",text="120ms",value=.25},{name="Volume",text="0.0dB",value=.75},
    {name="Filter",text="18.0kHz",value=.9},{name="Resonance",text="25%",value=.25},
    {name="Pan",text="Center",value=.5},{name="Drive",text="15%",value=.15}}},
  browser={kind=7,user=true,products={"Demo Library","Factory"},attributes={
    {values={"Drums","Bass"},chosen=1},{values={"Kick","Snare"},chosen=1},{values={"Warm","Dry"},chosen=2}},
    results={{name="Kick 909",favorite=true},{name="Snare Tight"},{name="Closed Hat"},{name="Shaker Dry"},{name="Clap Room"}},focus=2},
  sampling={tab=1,pages={
    {{name="Recording",knobs={{key="Rec.Mode",name="MODE",text="Detect",value=0,min=0,max=3,step=1,turn=.05},
      {key="Rec.Threshold",name="THRESHOLD",text="-12dB",value=-12,min=-60,max=0,step=1,turn=.01}}},
      {name="Source",knobs={{key="Rec.Source",name="SOURCE",text="External",value=0,min=0,max=2,step=1,turn=.05}}}},
    {{name="Play range",knobs={{key="Edit.Start",name="START",text="0.000s",value=0,min=0,max=48000,step=100,turn=.01},
      {key="Edit.End",name="END",text="1.000s",value=48000,min=0,max=48000,step=100,turn=.01}}},
      {name="Selection",knobs={{key="Sel.Start",name="START",text="0.250s",value=12000,min=0,max=48000,step=100,turn=.01}}}},
    {},{{name="Key / velocity",knobs={{key="Zone.Lo",name="KEY LO",text="C1",value=24,min=0,max=127,step=1,turn=.03},
      {key="Zone.Hi",name="KEY HI",text="B2",value=59,min=0,max=127,step=1,turn=.03},
      {key="Zone.VelLo",name="VEL LO",text="0",value=0,min=0,max=127,step=1,turn=.03},
      {key="Zone.VelHi",name="VEL HI",text="127",value=127,min=0,max=127,step=1,turn=.03}}}}},
    slice={
      ["Slice.Mode"]={key="Slice.Mode",name="MODE",text="Split",value=1,min=0,max=3,step=1,turn=.05,index=1},
      ["Slice.Split"]={key="Slice.Split",name="SLICES",text="8",value=8,min=1,max=64,step=1,turn=.03,index=2},
      ["Slice.Apply"]={key="Slice.Apply",name="APPLY",text="Zones",value=0,min=0,max=2,step=1,turn=.03,index=3}},
    record={waiting=false,recording=false,mode=0,threshold=-12,levels={.7,.4}},takes=3,take=1,
    sample={name="Break 123.wav",frames=48000,rate=48000,view={0,48000},play={0,48000},selection={12000,30000},looped={6000,42000},
      slices={0,6000,12000,18000,24000,30000,36000,42000},zones={{24,59,0,127},{60,95,32,110}},focus_zone=1}},
  playing=false,recording=false,follow=false,
}
local waves={overview={},detail={}}
for i=0,479 do
  local amplitude=(.15 + .8 * math.exp(-(i % 60) / 14)) * math.abs(math.sin(i * .19))
  for _,wave in pairs(waves) do wave[#wave+1]=-amplitude*.8; wave[#wave+1]=amplitude end
end
deckard={state=function() return state end, now=function() return now end, message=function() end,
  peaks=function(which) return waves[which] end,
  midi_send=function(role, bytes)
    assert(role=="controls")
    assert(#bytes <= 63 and bytes[1]==0xf0 and bytes[#bytes]==0xf7)
    for i=2,#bytes-1 do assert(bytes[i]>=0 and bytes[i]<128, "non-MIDI data byte") end
    if bytes[4]==0x47 and bytes[5]==1 then
      local x,y,w,h=bytes[7]+128*bytes[8],bytes[9]+128*bytes[10],bytes[11]+128*bytes[12],bytes[13]+128*bytes[14]
      assert(x>=0 and y>=0 and x+w<=255 and y+h<=64,"rectangle exceeds the physical framebuffer")
    elseif bytes[4]==0x47 and bytes[5]==3 then
      local x,y,h=bytes[7]+128*bytes[8],bytes[9],bytes[10]
      local count=(#bytes-11)/4
      assert(count>=1 and count<=13 and count%1==0 and x+count<=255 and y+h<=64,"wave columns exceed the framebuffer")
      for i=11,#bytes-1,4 do assert(bytes[i]<=bytes[i+1] and bytes[i+1]<h and bytes[i+2]+128*bytes[i+3]<=511) end
    elseif bytes[4]==0x47 and bytes[5]==2 then
      local x,y,scale=bytes[7]+128*bytes[8],bytes[9]+128*bytes[10],bytes[11]
      assert(x+(#bytes-12)*6*scale-scale<=255 and y+8*scale<=64,"text exceeds the physical framebuffer")
    end
    sent[#sent+1]=bytes
    if capture then capture:write("P ",table.concat(bytes,","),"\n") end
  end}
for _,name in ipairs({"focus_group","focus_sound","mute_sound","solo_sound","mute_group","solo_group","focus_scene","pick_pattern",
  "play","stop","record","restart","set_loop","library","sampling","show_plugins","level","browse_focus","slot","browse_load",
  "slot_edit","auto_write","next_page","previous_page","browse_user","browse_favorites","browse_kind","browse_product","browse_attribute",
  "browse_prehear","browse_set_favorite","tempo","swing","volume","turn_knob","note_repeat","add_group","grid","follow","song_view",
  "edit","erase_sound","reset_sound","copy_sound","step","new_pattern","pattern_edit","pattern_length","sampling_knob","sampling_call","record_sample","take"}) do
  deckard[name]=function(...) calls[#calls+1]={name,...} end
end
deckard.sampling_tab=function(n) calls[#calls+1]={"sampling_tab",n}; state.sampling.tab=n end
local function last(name)
  for i=#calls,1,-1 do if calls[i][1]==name then return calls[i] end end
end
local function packet(kind,id)
  for i=#sent,1,-1 do if sent[i][4]==kind and sent[i][5]==id then return sent[i] end end
end
local function cc(key,value) device.midi("controls",{0xb0,key,value}) end
local function press(key) cc(key,127); cc(key,0) end
local function pad(n,velocity) device.midi("controls",{0x90,35+n,velocity or 100}) end
local function frame(name)
  device.tick()
  if capture then capture:write("F ",name,"\n") end
end
local function has_text(value)
  for _,p in ipairs(sent) do
    if p[4]==0x47 and p[5]==2 then
      local chars={}; for i=12,#p-1 do chars[#chars+1]=string.char(p[i]) end
      if table.concat(chars):find(value,1,true) then return true end
    end
  end
end
device=dofile(root .. "/main.lua")
device.open(); frame("control")
assert(has_text("Joao") and has_text("Resonance"))
assert(packet(0x4b,48)[6] > 0 and packet(0x4b,49)[8] > 0)
local before=#sent; device.tick(); assert(#sent==before,"unchanged state must send nothing")
local knob=state.control.knobs[3]; state.control.knobs[3]={name="Very long parameter name",text="123456789012345",value=.9}; frame("control-param-change")
state.control.knobs[3]=knob; frame("control-param-return")
state.groups[1].sounds[1].color={0,255,0}; device.tick(); assert(packet(0x4b,48)[7]>0)
state.groups[1].sounds[1].color={255,0,0}
-- Secondary group emitters reflect the same colour and real playback state.
state.groups[1].scene_pattern=1; device.tick(); local dim=packet(0x4b,65)[6]; assert(dim>0 and packet(0x4b,65)[7]==0)
state.groups[1].playing_progress=0; device.tick(); local medium=packet(0x4b,65)[6]; assert(medium>dim)
state.groups[1].playing=true; device.tick(); assert(packet(0x4b,65)[6]>medium)
state.groups[1].playing_progress=nil
state.groups[1].muted=true; device.tick(); assert(packet(0x4b,65)[6]==0)
state.groups[1].muted=false; state.groups[1].playing=false; state.groups[1].scene_pattern=nil; device.tick(); assert(packet(0x4b,65)[6]==0)
pad(2); assert(last("focus_sound")[2]==2)
cc(33,127); pad(1); assert(last("mute_sound")[2]==1); press(80); assert(last("mute_group")[2]==1); frame("mute"); cc(33,0)
cc(29,127); pad(1); pad(2); assert(last("copy_sound")[2]==1 and last("copy_sound")[3]==2); cc(29,0)
cc(2,127); cc(49,127); pad(2); assert(last("reset_sound")[2]==2); cc(49,0); cc(2,0)
cc(16,126); cc(16,2); assert(last("turn_knob")[2]==1 and math.abs(last("turn_knob")[3]-4/127)<.000001)
press(26); frame("step"); assert(packet(0x4b,64)[6]==1)
local notes=state.notes; state.notes={}; frame("step-empty"); state.notes=notes; frame("step-return")
-- STEP toggles FOLLOW and leaves PATTERN's pad mode intact, stopped or playing.
press(32); device.tick(); assert(last("follow")[2]==true and packet(0x4b,64)[6]==1 and packet(0x4b,15)[6]==100)
state.pattern_position=5000; device.tick(); pad(1); assert(last("step")[2]==0,"stopped transport keeps the displayed page")
state.playing=true; frame("step-follow"); pad(1); assert(last("step")[2]==3840,"FOLLOW advances with the playhead")
state.pattern_position=120; device.tick(); pad(1); assert(last("step")[2]==0,"FOLLOW returns to the first page on loop")
state.pattern_position=90000; device.tick(); pad(1); assert(last("step")[2]==3840,"FOLLOW clamps at the pattern limit")
press(48); device.tick(); pad(1); assert(last("follow")[2]==false and last("step")[2]==0,"manual page buttons disable FOLLOW")
press(32); device.tick(); pad(1); assert(last("step")[2]==3840,"STEP resumes FOLLOW")
press(32); device.tick(); assert(last("follow")[2]==false and packet(0x4b,64)[6]==1)
state.playing=false; state.pattern_position=nil; press(48)
pad(2,110); assert(last("step")[2]==240 and last("step")[4]==110)
cc(30,127); pad(2); assert(last("focus_sound")[2]==2); state.sound=2; cc(30,0); frame("step-snare")
press(46); pad(1); assert(last("step")[2]==3840)
press(46); pad(1); assert(last("step")[2]==3840)
press(45); state.sound=1
state.pattern_start=100; state.pattern_length=480; device.tick(); pad(1); assert(last("step")[2]==240)
before=#calls; pad(3); assert(#calls==before,"out-of-pattern pad must do nothing")
state.pattern_start=0; state.pattern_length=7680
press(43); assert(last("follow")[2]==true)
press(42); assert(last("grid")[2]=="step" and last("grid")[3]==5)
press(39); frame("patterns"); assert(packet(0x4b,64)[6]==2)
pad(3); assert(last("pick_pattern")[2]==2); pad(2); assert(last("new_pattern")[2]==1 and last("new_pattern")[3]==2)
cc(16,10); assert(last("pattern_length")[2]==11520)
press(26); assert(packet(0x4b,64)[6]==1)
cc(49,127); now=1; cc(26,127); now=1.1; cc(26,0); cc(49,0); assert(packet(0x4b,64)[6]==2)
press(26); assert(packet(0x4b,64)[6]==1); press(26); assert(packet(0x4b,64)[6]==0)
press(25); frame("scenes"); pad(2); assert(last("focus_scene")[2]==2); press(27)
press(8); frame("browse"); assert(last("library")[2]==true and has_text("Snare Tight"))
press(41); assert(last("browse_user")[2]==true)
cc(15,120); cc(15,0); assert(last("browse_focus")[2]==1)
cc(17,0); cc(17,4); assert(last("browse_product")[2]==2)
cc(20,0); cc(20,2); assert(last("browse_attribute")[2]==1 and last("browse_attribute")[3]==2)
cc(49,127); press(45); assert(last("browse_set_favorite")); press(46); assert(last("browse_prehear")); cc(49,0)
press(36); assert(last("browse_load"))
local results=state.browser.results; state.browser.results={}; frame("browse-empty"); state.browser.results=results; frame("browse-return")
press(10); assert(last("note_repeat")[3]==false)
-- Opening sampling closes browsing and updates the hardware pad routing.
press(9); frame("record"); assert(last("sampling")[2]==true and last("library")[2]==false and packet(0x4b,64)[6]==1)
press(43); assert(last("record_sample")); pad(2); assert(last("take")[2]==2)
state.sampling.takes=35; state.sampling.take=17; device.tick(); pad(1); assert(last("take")[2]==17)
cc(49,127); press(46); cc(49,0); device.tick(); pad(1); assert(last("take")[2]==33)
state.sampling.takes=3; state.sampling.take=1; device.tick()
state.sampling.record.waiting=true; frame("record-waiting"); press(44); assert(last("record_sample")[2]==true)
state.sampling.record.waiting=false; state.sampling.record.recording=true; frame("record-active")
before=#calls; cc(16,18); assert(#calls==before,"recorder parameters must stay fixed while recording")
state.sampling.record.recording=false
press(47); cc(16,28); assert(last("sampling_knob")[2]==2 and last("sampling_knob")[3]==1)
press(48); cc(16,38); local first=last("sampling_knob")[4]; cc(16,48); assert(last("sampling_knob")[4]>=first,"pending turns must accumulate")
press(40); frame("edit"); assert(last("sampling_tab")[2]==2 and packet(0x4b,64)[6]==0)
press(46); assert(last("sampling_call")[2]=="edit" and last("sampling_call")[3]==1)
press(44); press(46); assert(last("sampling_call")[3]==2)
cc(20,10); assert(last("sampling_call")[2]=="view" and last("sampling_call")[4]<48000)
cc(49,127); press(36); cc(49,0); assert(last("sampling_call")[2]=="view" and last("sampling_call")[3]==0 and last("sampling_call")[4]==48000)
state.sampling.sample.view={6000,30000}; state.sampling.sample.playhead=10000; frame("edit-zoom")
before=#sent; state.sampling.sample.playhead=11000; device.tick()
local columns=0
for i=before+1,#sent do
  local p=sent[i]
  if p[4]==0x47 and p[5]==3 then columns=columns+(#p-11)/4 end
  assert(not (p[4]==0x47 and p[5]==1 and p[15]==5),"moving a wave cursor must preserve surrounding regions")
end
assert(columns>0 and columns<=4,"cursor repaints old/new columns on both screens")
frame("edit-cursor")
before=#sent; device.tick(); assert(#sent==before)
press(41); frame("slice"); assert(packet(0x4b,64)[6]==1)
pad(3); assert(last("sampling_call")[2]=="prehear" and last("sampling_call")[3]==12000)
press(43); assert(last("sampling_call")[2]=="split_slice" and last("sampling_call")[3]==3)
press(47); cc(16,56); assert(last("sampling_call")[2]~="move_slice")
cc(18,0); cc(18,4); assert(last("sampling_call")[2]=="move_slice")
press(47); cc(17,12); assert(last("sampling_knob")[3]==3,"SLICE sends the API index, not its page position")
press(46); assert(last("sampling_call")[2]=="apply_slices")
press(42); frame("zones"); assert(packet(0x4b,64)[6]==0)
press(46); assert(last("sampling_call")[2]=="focus_zone" and last("sampling_call")[3]==2)
before=#calls; press(44); assert(#calls==before,"unsupported ADD ZONE stays disabled")
-- Transport, modifiers, mode buttons and SAMPLING itself remain reachable.
press(1); assert(last("play")); cc(49,127); cc(49,0); assert(last("sampling")[2]==true)
press(9); device.tick(); assert(last("sampling")[2]==false and packet(0x4b,64)[6]==0)
press(9); press(11); assert(last("sampling")[2]==false and last("level")[2]==3)
press(9); press(32); device.tick(); assert(last("sampling")[2]==true and packet(0x4b,64)[6]==0)
press(32); press(9)
-- Missing sample/peaks and asynchronous re-entry clear stale drawings.
press(9); state.sampling.tab=2
local sample=state.sampling.sample; state.sampling.sample=nil; frame("edit-empty"); state.sampling.sample=sample
local previous_waves=waves; waves={overview={},detail={}}; frame("edit-no-peaks"); assert(has_text("WAVEFORM UNAVAILABLE")); waves=previous_waves; frame("edit-restored")
state.sampling.stems={focused=true,progress=.42}; frame("stems"); press(46); assert(last("sampling_call")[2]=="cancel_stems"); state.sampling.stems=nil
press(9); frame("control-return")
device.close()
for id=65,72 do local p=packet(0x4b,id); assert(p[6]==0 and p[7]==0 and p[8]==0) end
if capture then capture:close() end
print("Maschine MK2: graphics, RGB, step/pattern/scene, browser and all sampling tabs verified")
