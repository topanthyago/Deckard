-- Controls and musical notes use separate ports so pad modes stay silent.
return {
  name = "Maschine MK2",
  ports = { controls = { "Controls MIDI" } },
  exclusive = { "controls" },
  drums = { "Pads MIDI" },
}
