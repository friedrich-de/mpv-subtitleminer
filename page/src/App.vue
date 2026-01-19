<script setup lang="ts">
  import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
  import { useToast } from './composables/useToast'
  import { useWebSocket } from './composables/useWebSocket'
  import * as anki from './services/ankiConnect'
  import { isJsonObject, type JsonObject, type JsonValue } from './types/json'

  const DEFAULT_PORTS = [61777, 61778, 61779, 61780, 61781]

  const { toasts, toast, toastIcons, dismissToast } = useToast()

  interface AnkiSettings {
    noteType: string
    frontField: string
    sentenceField: string
    audioField: string
    imageField: string
  }

  interface ConnectionSettings {
    host: string
    ports: number[]
  }

  type ImageFormat = 'jpg' | 'webp' | 'avif'
  type AudioFormat = 'mp3' | 'opus'

  interface MediaSettings {
    image: {
      format: ImageFormat
      quality: number
    }
    audio: {
      format: AudioFormat
      bitrate: string
      filters: string
    }
  }

  interface Settings {
    anki: AnkiSettings
    connection: ConnectionSettings
    media: MediaSettings
  }

  const STORAGE_KEY = 'mpv_subtitle_tool_settings'
  const defaultSettings: Settings = {
    anki: { noteType: '', frontField: '', sentenceField: '', audioField: '', imageField: '' },
    connection: { host: '127.0.0.1', ports: [...DEFAULT_PORTS] },
    media: {
      image: { format: 'jpg', quality: 80 },
      audio: { format: 'mp3', bitrate: '', filters: '' },
    },
  }

  function loadSettings(): Settings {
    try {
      const stored = localStorage.getItem(STORAGE_KEY)
      if (stored) {
        const parsed = JSON.parse(stored) as Settings
        return {
          ...defaultSettings,
          ...parsed,
          anki: { ...defaultSettings.anki, ...parsed.anki },
          connection: { ...defaultSettings.connection, ...parsed.connection },
          media: {
            ...defaultSettings.media,
            ...parsed.media,
            image: { ...defaultSettings.media.image, ...parsed.media?.image },
            audio: { ...defaultSettings.media.audio, ...parsed.media?.audio },
          },
        }
      }
    } catch (err) {
      console.warn('Failed to load settings', err)
    }
    return { ...defaultSettings }
  }

  const settings = ref<Settings>(loadSettings())

  const cloneMediaSettings = (source: MediaSettings): MediaSettings => ({
    image: { ...source.image },
    audio: { ...source.audio },
  })

  watch(
    settings,
    (value) => {
      try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(value))
      } catch (err) {
        console.warn('Failed to save settings', err)
      }
    },
    { deep: true },
  )

  const ankiConfigured = computed(() => {
    const { noteType, sentenceField, audioField, imageField } = settings.value.anki
    return !!noteType && (!!sentenceField || !!audioField || !!imageField)
  })

  const showSettings = ref(false)
  type ConnectionStatus = 'untested' | 'testing' | 'connected' | 'error'
  const connectionStatus = ref<ConnectionStatus>('untested')
  const ankiVersion = ref<number | null>(null)
  const connectionError = ref<string | null>(null)
  const modelsWithFields = ref<Record<string, string[]>>({})
  const loadingModels = ref(false)
  const modelsError = ref<string | null>(null)
  const localSettings = ref<AnkiSettings>({ ...settings.value.anki })
  const localMediaSettings = ref<MediaSettings>(cloneMediaSettings(settings.value.media))

  const modelNames = computed(() => Object.keys(modelsWithFields.value).sort())
  const availableFields = computed(() => {
    const model = localSettings.value.noteType
    return model ? (modelsWithFields.value[model] ?? []) : []
  })
  watch(showSettings, (isOpen) => {
    if (isOpen) {
      localSettings.value = { ...settings.value.anki }
      localMediaSettings.value = cloneMediaSettings(settings.value.media)
      if (connectionStatus.value === 'untested') {
        void testConnection()
      }
    }
  })

  async function testConnection() {
    connectionStatus.value = 'testing'
    connectionError.value = null

    try {
      ankiVersion.value = await anki.getVersion()
      connectionStatus.value = 'connected'
      await loadModels()
    } catch (err) {
      connectionStatus.value = 'error'
      connectionError.value = err instanceof Error ? err.message : 'Unknown error'
    }
  }

  async function loadModels() {
    if (loadingModels.value) return

    loadingModels.value = true
    modelsError.value = null

    try {
      modelsWithFields.value = await anki.getModelsWithFields()
    } catch (err) {
      modelsError.value = err instanceof Error ? err.message : 'Failed to load models'
    } finally {
      loadingModels.value = false
    }
  }

  function onModelChange(value: string) {
    localSettings.value = {
      ...localSettings.value,
      noteType: value,
      frontField: '',
      sentenceField: '',
      audioField: '',
      imageField: '',
    }
  }

  function onFieldChange(field: keyof AnkiSettings, value: string) {
    localSettings.value = { ...localSettings.value, [field]: value }
  }

  function saveSettings() {
    settings.value.anki = { ...localSettings.value }
    settings.value.media = cloneMediaSettings(localMediaSettings.value)
    showSettings.value = false
    toast.success('Settings saved')
  }

  function cancelSettings() {
    showSettings.value = false
  }

  interface SubtitleMessage {
    id: number
    subtitle: string
    time_pos: number
    sub_start: number
    sub_end: number
    thumbnail?: string
    thumbnailExt?: string
    thumbnailMime?: string
    audio?: string
    audioExt?: string
    audioMime?: string
    sourcePort: number
    uid: string
  }

  type MediaInfo = {
    data: string
    ext: string
    mime: string
  }

  type AudioRangeInfo = MediaInfo & {
    startId: number
    endId: number
  }

  const messages = ref<SubtitleMessage[]>([])
  const bottomRef = ref<HTMLElement | null>(null)
  const hoveredThumbnailUid = ref<string | null>(null)
  const loadingMedia = ref<Record<string, boolean>>({})
  const selectedMessages = ref<Set<string>>(new Set())
  const currentAudio = ref<HTMLAudioElement | null>(null)
  const pendingAudioRange = ref<(AudioRangeInfo & { port: number }) | null>(null)
  const selectionBarRef = ref<HTMLElement | null>(null)
  const selectionBarHeight = ref(0)
  let selectionBarObserver: ResizeObserver | null = null
  const sendingToAnki = ref<Record<string, boolean>>({})
  const ankiSuccess = ref<Record<string, boolean>>({})
  const ankiError = ref<Record<string, string>>({})
  const targetCardPreview = ref<string | null>(null)
  const loadingTargetCard = ref(false)

  const host = computed({
    get: () => settings.value.connection.host,
    set: (value: string) => {
      settings.value.connection.host = value
    },
  })
  const ports = computed({
    get: () => settings.value.connection.ports,
    set: (value: number[]) => {
      settings.value.connection.ports = value
    },
  })
  const portInput = ref(ports.value.join(', '))

  watch(ports, (newPorts) => {
    portInput.value = newPorts.join(', ')
  })

  function updatePorts(raw: string) {
    portInput.value = raw
    const parsed = raw
      .split(/[\s,]+/)
      .map((v) => parseInt(v, 10))
      .filter((n) => Number.isInteger(n) && n > 0 && n <= 65535)
    if (parsed.length) {
      ports.value = parsed
    }
  }

  function resetConnectionDefaults() {
    host.value = defaultSettings.connection.host
    ports.value = [...DEFAULT_PORTS]
  }

  const ws = useWebSocket({
    host,
    ports,
    retryDelay: 1000,
    onMessage: (data: JsonValue, port: number) => {
      if (!isJsonObject(data)) return
      const type = data.type
      if (typeof type !== 'string') return
      const d = data

      if (type === 'subtitle') {
        const msg = parseSubtitleMessage(d, port)
        if (!msg) return
        messages.value.push(msg)
        if (messages.value.length > 200) messages.value.shift()
        void nextTick(() => bottomRef.value?.scrollIntoView({ block: 'end' }))
        return
      }

      if (type === 'thumbnail' || type === 'audio') {
        const media = parseMediaMessage(d, type)
        if (!media) return

        const msg = messages.value.find((m) => m.id === media.id && m.sourcePort === port)
        if (msg) {
          if (type === 'thumbnail') {
            msg.thumbnail = media.data
            msg.thumbnailExt = media.ext
            msg.thumbnailMime = media.mime
          } else {
            msg.audio = media.data
            msg.audioExt = media.ext
            msg.audioMime = media.mime
            playAudio(media.data, media.mime)
          }
        }
        const key = `${type === 'thumbnail' ? 'thumb' : 'audio'}-${port}-${media.id}`
        delete loadingMedia.value[key]
        return
      }

      if (type === 'audio_range') {
        const range = parseAudioRangeMessage(d)
        if (!range) return

        pendingAudioRange.value = { ...range, port }
        const key = `audio_range-${port}-${range.startId}-${range.endId}`
        delete loadingMedia.value[key]
      }
    },
    onStatusChange: (status, _port, message) => {
      if (status === 'connected') {
        toast.success(message)
      } else if (status === 'connecting') {
        toast.info(message)
      }
    },
  })

  const updateSelectionBarHeight = () => {
    selectionBarHeight.value = selectionBarRef.value?.offsetHeight ?? 0
  }

  onMounted(() => {
    ws.connect()
    updateSelectionBarHeight()
    if (typeof ResizeObserver !== 'undefined') {
      selectionBarObserver = new ResizeObserver(() => updateSelectionBarHeight())
      if (selectionBarRef.value) {
        selectionBarObserver.observe(selectionBarRef.value)
      }
    } else if (typeof window !== 'undefined') {
      window.addEventListener('resize', updateSelectionBarHeight)
    }
  })

  onBeforeUnmount(() => {
    if (selectionBarObserver) {
      selectionBarObserver.disconnect()
      selectionBarObserver = null
      return
    }
    if (typeof window !== 'undefined') {
      window.removeEventListener('resize', updateSelectionBarHeight)
    }
  })

  function asNumber(value: JsonValue | undefined): number | null {
    return typeof value === 'number' && Number.isFinite(value) ? value : null
  }

  function asString(value: JsonValue | undefined): string | null {
    return typeof value === 'string' ? value : null
  }

  function parseSubtitleMessage(d: JsonObject, port: number): SubtitleMessage | null {
    const id = asNumber(d.id)
    const subtitle = asString(d.subtitle)
    const sub_start = asNumber(d.sub_start)
    const sub_end = asNumber(d.sub_end)
    const time_pos = asNumber(d.time_pos)
    if (id === null || subtitle === null || sub_start === null || sub_end === null) {
      return null
    }
    const normalizedTimePos = time_pos ?? sub_start
    const uid = `${port}-${id}`
    return { id, subtitle, time_pos: normalizedTimePos, sub_start, sub_end, sourcePort: port, uid }
  }

  function parseMediaMessage(
    d: JsonObject,
    type: 'thumbnail' | 'audio',
  ): ({ id: number } & MediaInfo) | null {
    const id = asNumber(d.id)
    const data = asString(d.data)
    if (id === null || data === null) return null
    const ext = asString(d.ext)
    const mime = asString(d.mime)
    if (type === 'thumbnail') {
      const format = settings.value.media.image.format
      return {
        id,
        data,
        ext: ext ?? format,
        mime: mime ?? imageMimeMap[format],
      }
    }
    const format = settings.value.media.audio.format
    return {
      id,
      data,
      ext: ext ?? format,
      mime: mime ?? audioMimeMap[format],
    }
  }

  function parseAudioRangeMessage(d: JsonObject): AudioRangeInfo | null {
    const startId = asNumber(d.start_id)
    const endId = asNumber(d.end_id)
    const data = asString(d.data)
    if (startId === null || endId === null || data === null) return null
    const ext = asString(d.ext)
    const mime = asString(d.mime)
    const format = settings.value.media.audio.format
    return {
      startId,
      endId,
      data,
      ext: ext ?? format,
      mime: mime ?? audioMimeMap[format],
    }
  }

  const imageMimeMap: Record<ImageFormat, string> = {
    jpg: 'image/jpeg',
    webp: 'image/webp',
    avif: 'image/avif',
  }
  const audioMimeMap: Record<AudioFormat, string> = {
    mp3: 'audio/mpeg',
    opus: 'audio/ogg',
  }

  const getMessageMediaInfo = (
    msg: SubtitleMessage,
    type: 'thumbnail' | 'audio',
  ): MediaInfo | undefined => {
    if (type === 'thumbnail' && msg.thumbnail) {
      const format = settings.value.media.image.format
      return {
        data: msg.thumbnail,
        ext: msg.thumbnailExt ?? format,
        mime: msg.thumbnailMime ?? imageMimeMap[format],
      }
    }
    if (type === 'audio' && msg.audio) {
      const format = settings.value.media.audio.format
      return {
        data: msg.audio,
        ext: msg.audioExt ?? format,
        mime: msg.audioMime ?? audioMimeMap[format],
      }
    }
    return undefined
  }

  const buildThumbnailRequest = (id: number): JsonObject => {
    const { format, quality } = settings.value.media.image
    const payload: JsonObject = { request: 'thumbnail', id, image_format: format }
    if (format === 'webp' && Number.isFinite(quality) && quality >= 0 && quality <= 100) {
      payload.image_quality = Math.round(quality)
    }
    return payload
  }

  const buildAudioRequest = (id: number): JsonObject => {
    const { format, bitrate, filters } = settings.value.media.audio
    const payload: JsonObject = { request: 'audio', id, audio_format: format }
    const trimmedBitrate = bitrate.trim()
    if (trimmedBitrate) {
      payload.audio_bitrate = trimmedBitrate
    }
    const trimmedFilters = filters.trim()
    if (trimmedFilters) {
      payload.audio_filters = trimmedFilters
    }
    return payload
  }

  const buildAudioRangeRequest = (startId: number, endId: number): JsonObject => {
    const { format, bitrate, filters } = settings.value.media.audio
    const payload: JsonObject = {
      request: 'audio_range',
      start_id: startId,
      end_id: endId,
      audio_format: format,
    }
    const trimmedBitrate = bitrate.trim()
    if (trimmedBitrate) {
      payload.audio_bitrate = trimmedBitrate
    }
    const trimmedFilters = filters.trim()
    if (trimmedFilters) {
      payload.audio_filters = trimmedFilters
    }
    return payload
  }

  const isSelected = (uid: string) => selectedMessages.value.has(uid)

  const toggleSelection = (msg: SubtitleMessage, index: number) => {
    const id = msg.uid
    const selected = selectedMessages.value

    if (selected.has(id)) {
      const selectedIndices = Array.from(selected)
        .map((selId) => messages.value.findIndex((m) => m.uid === selId))
        .filter((i) => i !== -1)
        .sort((a, b) => a - b)

      if (index === selectedIndices[0] || index === selectedIndices[selectedIndices.length - 1]) {
        selected.delete(id)
      }
    } else {
      if (selected.size === 0) {
        selected.add(id)
      } else {
        const selectedIndices = Array.from(selected)
          .map((selId) => messages.value.findIndex((m) => m.uid === selId))
          .filter((i) => i !== -1)
          .sort((a, b) => a - b)

        const minIdx = selectedIndices[0] ?? index
        const maxIdx = selectedIndices[selectedIndices.length - 1] ?? index

        if (index === minIdx - 1 || index === maxIdx + 1) {
          selected.add(id)
        }
      }
    }
    selectedMessages.value = new Set(selected)
  }

  const clearSelection = () => {
    selectedMessages.value = new Set()
  }

  const getSelectedMessages = (): SubtitleMessage[] => {
    return messages.value.filter((m) => selectedMessages.value.has(m.uid))
  }

  const getSelectionRange = (): { first: SubtitleMessage; last: SubtitleMessage } | null => {
    const selected = getSelectedMessages()
    if (selected.length === 0) return null

    const sorted = selected.sort((a, b) => {
      const aIdx = messages.value.findIndex((m) => m.uid === a.uid)
      const bIdx = messages.value.findIndex((m) => m.uid === b.uid)
      return aIdx - bIdx
    })

    const first = sorted[0]
    const last = sorted[sorted.length - 1]
    if (!first || !last) return null

    return { first, last }
  }

  const selectionRange = computed(() => {
    if (selectedMessages.value.size < 2) return null
    return getSelectionRange()
  })

  const selectionRangeAnchorUid = computed(() => selectionRange.value?.last.uid ?? null)

  const selectionAudioKey = computed(() => {
    const range = selectionRange.value
    if (!range) return null
    return `audio_range-${range.first.sourcePort}-${range.first.id}-${range.last.id}`
  })

  const selectionAudioLoading = computed(() => {
    const key = selectionAudioKey.value
    return key ? !!loadingMedia.value[key] : false
  })

  const updateTargetCardPreview = async () => {
    if (selectedMessages.value.size === 0 || !ankiConfigured.value) {
      targetCardPreview.value = null
      return
    }

    loadingTargetCard.value = true
    try {
      const { noteType, frontField } = settings.value.anki
      const targetNote = await anki.getLastNote(noteType)

      if (targetNote) {
        let preview = ''
        if (frontField && targetNote.fields[frontField]) {
          preview = targetNote.fields[frontField].value
        } else {
          for (const field of Object.values(targetNote.fields)) {
            if (field.value) {
              preview = field.value
              break
            }
          }
        }
        preview = preview.replace(/<[^>]*>/g, '').trim()
        if (preview.length > 50) {
          preview = preview.slice(0, 50) + '…'
        }
        targetCardPreview.value = preview || `Note #${targetNote.noteId}`
      } else {
        targetCardPreview.value = null
      }
    } catch {
      targetCardPreview.value = null
    } finally {
      loadingTargetCard.value = false
    }
  }

  watch(
    () => selectedMessages.value.size,
    () => updateTargetCardPreview(),
    { immediate: true },
  )

  const sendToPort = (payload: JsonValue, port: number | undefined): boolean => {
    if (!port) return false
    return ws.send(payload, port)
  }

  const requestThumbnail = (msg: SubtitleMessage) => {
    if (ws.status.value !== 'connected' || msg.thumbnail) return
    const key = `thumb-${msg.uid}`
    if (loadingMedia.value[key]) return
    if (!sendToPort(buildThumbnailRequest(msg.id), msg.sourcePort)) {
      toast.error(`Not connected to port ${msg.sourcePort}`)
      return
    }
    loadingMedia.value[key] = true
  }

  const requestAudio = (msg: SubtitleMessage) => {
    if (ws.status.value !== 'connected') return
    if (msg.audio) {
      playAudio(msg.audio, msg.audioMime)
      return
    }
    const key = `audio-${msg.uid}`
    if (loadingMedia.value[key]) return
    if (!sendToPort(buildAudioRequest(msg.id), msg.sourcePort)) {
      toast.error(`Not connected to port ${msg.sourcePort}`)
      return
    }
    loadingMedia.value[key] = true
  }

  const requestSelectionAudioRange = async () => {
    const range = selectionRange.value
    if (!range) return

    const selectedMsgs = getSelectedMessages()
    const selectionPort = range.first.sourcePort
    const allSamePort = selectedMsgs.every((msg) => msg.sourcePort === selectionPort)
    if (!allSamePort) {
      toast.error('Selected subtitles must come from the same connection for audio.')
      return
    }

    const audioData = await requestAudioRange(range.first.id, range.last.id, selectionPort)
    if (audioData) {
      playAudio(audioData.data, audioData.mime)
    }
  }

  const playAudio = (audioBase64: string, mime?: string) => {
    if (currentAudio.value) {
      currentAudio.value.pause()
      currentAudio.value = null
    }
    const resolvedMime = mime ?? audioMimeMap[settings.value.media.audio.format]
    const audio = new Audio(`data:${resolvedMime};base64,${audioBase64}`)
    currentAudio.value = audio
    audio.addEventListener('ended', () => {
      if (currentAudio.value === audio) {
        currentAudio.value = null
      }
    })
    void audio.play()
  }

  const generateMediaFilename = (msgId: number, ext: string) => {
    const timestamp = Date.now()
    return `mpv_subtitleminer_${msgId}_${timestamp}.${ext}`
  }

  const sendSelectionToAnki = async () => {
    const selectedMsgs = getSelectedMessages()
    if (!ankiConfigured.value || selectedMsgs.length === 0) return

    const { sentenceField, audioField, imageField } = settings.value.anki
    const { first, last } = getSelectionRange() ?? {}
    if (!first || !last) return

    const primaryKey = first.uid
    const primaryId = first.id
    sendingToAnki.value[primaryKey] = true
    ankiError.value[primaryKey] = ''

    try {
      if (!ankiConfigured.value) {
        throw new Error('Anki settings are incomplete')
      }

      const targetNote = await anki.getLastNote(settings.value.anki.noteType)
      if (!targetNote) {
        throw new Error('No target card found in Anki')
      }

      const fieldUpdates: Record<string, string> = {}

      if (sentenceField) {
        const text = selectedMsgs.map((m) => m.subtitle).join(' ')
        fieldUpdates[sentenceField] = text
      }

      if (audioField) {
        if (selectedMsgs.length > 1) {
          const selectionPort = first.sourcePort
          const allSamePort = selectedMsgs.every((msg) => msg.sourcePort === selectionPort)
          if (!allSamePort) {
            throw new Error('Selected subtitles must come from the same connection for audio.')
          }
        }
        let audioData =
          selectedMsgs.length > 1
            ? await requestAudioRange(first.id, last.id, first.sourcePort)
            : getMessageMediaInfo(first, 'audio') ?? (await requestMediaFromServer(first, 'audio'))

        if (audioData) {
          const filename = generateMediaFilename(primaryId, audioData.ext)
          await anki.storeMediaFile(filename, audioData.data)
          fieldUpdates[audioField] = `[sound:${filename}]`
        }
      }

      if (imageField) {
        let imageData = getMessageMediaInfo(first, 'thumbnail')
        if (!imageData) {
          imageData = await requestMediaFromServer(first, 'thumbnail')
        }
        if (imageData) {
          const filename = generateMediaFilename(primaryId, imageData.ext)
          await anki.storeMediaFile(filename, imageData.data)
          fieldUpdates[imageField] = `<img src="${filename}">`
        }
      }

      if (Object.keys(fieldUpdates).length > 0) {
        await anki.updateNoteFields(targetNote.noteId, fieldUpdates)
        ankiSuccess.value[primaryKey] = true
        const noteId = targetNote.noteId
        toast.success(`Added ${selectedMsgs.length} subtitle(s) to Anki`, {
          duration: 5000,
          action: {
            label: 'Browse',
            onClick: () => {
              void anki.guiBrowse(`nid:${noteId}`)
            },
          },
        })

        setTimeout(() => {
          delete ankiSuccess.value[primaryKey]
          clearSelection()
        }, 2000)
      }
    } catch (err) {
      ankiError.value[primaryKey] = err instanceof Error ? err.message : 'Unknown error'
      toast.error(err instanceof Error ? err.message : 'Failed to add to Anki')
    } finally {
      delete sendingToAnki.value[primaryKey]
    }
  }

  const requestMediaFromServer = (
    msg: SubtitleMessage,
    type: 'audio' | 'thumbnail',
  ): Promise<MediaInfo | undefined> => {
    return new Promise((resolve) => {
      if (ws.status.value !== 'connected') {
        resolve(undefined)
        return
      }

      const key = `${type === 'thumbnail' ? 'thumb' : 'audio'}-${msg.uid}`
      if (loadingMedia.value[key]) {
        const checkInterval = setInterval(() => {
          const media = getMessageMediaInfo(msg, type)
          if (media) {
            clearInterval(checkInterval)
            resolve(media)
          } else if (!loadingMedia.value[key]) {
            clearInterval(checkInterval)
            resolve(undefined)
          }
        }, 100)

        setTimeout(() => {
          clearInterval(checkInterval)
          resolve(getMessageMediaInfo(msg, type) ?? undefined)
        }, 10000)
        return
      }

      loadingMedia.value[key] = true
      const payload = type === 'thumbnail' ? buildThumbnailRequest(msg.id) : buildAudioRequest(msg.id)
      if (!sendToPort(payload, msg.sourcePort)) {
        delete loadingMedia.value[key]
        resolve(undefined)
        return
      }

      const checkInterval = setInterval(() => {
        const media = getMessageMediaInfo(msg, type)
        if (media) {
          clearInterval(checkInterval)
          resolve(media)
        }
      }, 100)

      setTimeout(() => {
        clearInterval(checkInterval)
        delete loadingMedia.value[key]
        resolve(getMessageMediaInfo(msg, type) ?? undefined)
      }, 10000)
    })
  }

  const requestAudioRange = (
    startId: number,
    endId: number,
    port: number,
  ): Promise<MediaInfo | undefined> => {
    return new Promise((resolve) => {
      if (ws.status.value !== 'connected') {
        resolve(undefined)
        return
      }

      const key = `audio_range-${port}-${startId}-${endId}`
      if (loadingMedia.value[key]) {
        const checkInterval = setInterval(() => {
          const result = pendingAudioRange.value
          if (
            result &&
            result.startId === startId &&
            result.endId === endId &&
            result.port === port
          ) {
            clearInterval(checkInterval)
            pendingAudioRange.value = null
            resolve({ data: result.data, ext: result.ext, mime: result.mime })
          } else if (!loadingMedia.value[key]) {
            clearInterval(checkInterval)
            resolve(undefined)
          }
        }, 100)

        setTimeout(() => {
          clearInterval(checkInterval)
          resolve(undefined)
        }, 15000)
        return
      }

      loadingMedia.value[key] = true
      if (!sendToPort(buildAudioRangeRequest(startId, endId), port)) {
        delete loadingMedia.value[key]
        resolve(undefined)
        return
      }

      const checkInterval = setInterval(() => {
        const result = pendingAudioRange.value
        if (
          result &&
          result.startId === startId &&
          result.endId === endId &&
          result.port === port
        ) {
          clearInterval(checkInterval)
          pendingAudioRange.value = null
          resolve({ data: result.data, ext: result.ext, mime: result.mime })
        }
      }, 100)

      setTimeout(() => {
        clearInterval(checkInterval)
        delete loadingMedia.value[key]
        resolve(undefined)
      }, 15000)
    })
  }
</script>

<template>
  <div class="app" :style="{ '--selection-bar-height': `${selectionBarHeight}px` }">
    <header class="topbar">
      <div class="brand">
        <span class="title">MPV Subtitle Tool</span>
        <span class="status" :data-state="ws.status.value">
          <span class="dot" aria-hidden="true"></span>
          <span class="label">{{ ws.status.value }}</span>
          <span v-if="ws.connectedPorts.value.length" class="port">
            ({{ ws.connectedPorts.value.join(', ') }})
          </span>
        </span>
      </div>
      <div class="controls">
        <label class="field">
          <span>IP</span>
          <input v-model="host" type="text" class="input" />
        </label>
        <label class="field">
          <span>Ports</span>
          <input
            :value="portInput"
            type="text"
            class="input"
            placeholder="61777, 61778"
            @input="(e) => updatePorts((e.target as HTMLInputElement).value)"
          />
        </label>
        <button class="btn" type="button" @click="ws.connect">Connect</button>
        <button class="btn ghost" type="button" @click="ws.disconnect">Disconnect</button>
        <button class="btn ghost" type="button" @click="resetConnectionDefaults">
          Reset Connection
        </button>
        <button class="btn ghost" type="button" @click="showSettings = true">⚙ Settings</button>
      </div>
    </header>

    <main class="main">
      <div v-if="messages.length === 0" class="empty">Waiting for subtitles...</div>
      <ul v-else class="messages">
        <li
          v-for="(message, index) in messages"
          :key="message.uid"
          class="message-row"
          :class="{ selected: isSelected(message.uid) }"
          @click="toggleSelection(message, index)"
        >
          <span class="subtitle-text">{{ message.subtitle }}</span>
          <div class="actions">
            <div class="thumb-action">
              <button
                class="icon-btn"
                :class="{
                  loading: loadingMedia[`thumb-${message.uid}`],
                  active: message.thumbnail,
                }"
                title="Screenshot"
                @click.stop="requestThumbnail(message)"
                @mouseenter="hoveredThumbnailUid = message.uid"
                @mouseleave="
                  () => {
                    if (hoveredThumbnailUid === message.uid) {
                      hoveredThumbnailUid = null
                    }
                  }
                "
              >
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="2"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                >
                  <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
                  <circle cx="8.5" cy="8.5" r="1.5" />
                  <polyline points="21 15 16 10 5 21" />
                </svg>
              </button>
              <div
                v-if="hoveredThumbnailUid === message.uid && message.thumbnail"
                class="thumb-preview"
              >
                <img
                  :src="`data:${message.thumbnailMime ?? imageMimeMap[settings.media.image.format]};base64,${message.thumbnail}`"
                  alt="Thumbnail"
                />
              </div>
            </div>
            <button
              class="icon-btn"
              :class="{ loading: loadingMedia[`audio-${message.uid}`], active: message.audio }"
              title="Play audio"
              @click.stop="requestAudio(message)"
            >
              <svg
                xmlns="http://www.w3.org/2000/svg"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" />
                <path d="M15.54 8.46a5 5 0 0 1 0 7.07" />
                <path d="M19.07 4.93a10 10 0 0 1 0 14.14" />
              </svg>
            </button>
          </div>
          <button
            v-if="selectionRangeAnchorUid === message.uid"
            class="icon-btn range-audio-btn"
            :class="{ loading: selectionAudioLoading }"
            :disabled="selectionAudioLoading"
            title="Play selected range"
            @click.stop="requestSelectionAudioRange"
          >
            <svg
              xmlns="http://www.w3.org/2000/svg"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            >
              <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" />
              <path d="M15.54 8.46a5 5 0 0 1 0 7.07" />
              <path d="M19.07 4.93a10 10 0 0 1 0 14.14" />
            </svg>
          </button>
        </li>
      </ul>
      <div ref="bottomRef" class="bottom-anchor" aria-hidden="true"></div>
    </main>

    <div
      ref="selectionBarRef"
      class="selection-bar"
      :class="{ inactive: selectedMessages.size === 0 }"
    >
      <div class="selection-left">
        <template v-if="selectedMessages.size > 0">
          <span class="selection-count">{{ selectedMessages.size }} selected</span>
          <span v-if="loadingTargetCard" class="target-card loading">Loading card...</span>
          <span v-else-if="targetCardPreview" class="target-card" title="Target card">
            → {{ targetCardPreview }}
          </span>
          <span v-else-if="ankiConfigured" class="target-card error">No matching card found</span>
        </template>
        <span v-else class="selection-hint">Click on subtitles to select them for Anki</span>
      </div>
      <div class="selection-right">
        <button
          class="selection-btn send-btn"
          :disabled="!targetCardPreview || selectedMessages.size === 0"
          @click="sendSelectionToAnki"
        >
          📝 Add to Anki
        </button>
        <button
          class="selection-btn clear-btn"
          :disabled="selectedMessages.size === 0"
          @click="clearSelection"
        >
          ✕ Clear
        </button>
      </div>
    </div>

    <Teleport to="body">
      <div v-if="showSettings" class="modal-overlay" @click.self="cancelSettings">
        <div class="modal">
          <header class="modal-header">
            <h2>Settings</h2>
            <button class="icon-btn close" aria-label="Close" @click="cancelSettings">×</button>
          </header>

          <div class="modal-body">
            <section class="section">
              <div class="section-header">
                <h3>AnkiConnect</h3>
                <button
                  class="btn"
                  :class="{ muted: connectionStatus === 'testing' }"
                  :disabled="connectionStatus === 'testing'"
                  @click="testConnection"
                >
                  {{ connectionStatus === 'testing' ? 'Testing…' : 'Test connection' }}
                </button>
              </div>

              <div class="connection-row">
                <span v-if="connectionStatus === 'connected'" class="status-pill success"
                  >✓ Connected (v{{ ankiVersion }})</span
                >
                <span
                  v-else-if="connectionStatus === 'error'"
                  class="status-pill error"
                  :title="connectionError ?? ''"
                  >✗ {{ connectionError }}</span
                >
                <span v-else class="status-pill">Not tested</span>
              </div>
              <p class="hint">AnkiConnect must be installed and reachable on port 8765.</p>
            </section>

            <section class="section">
              <div class="section-header">
                <h3>Card configuration</h3>
                <span v-if="connectionStatus !== 'connected'" class="subtle"
                  >Connect first to load models</span
                >
              </div>

              <div v-if="connectionStatus !== 'connected'" class="muted-box">
                Connect to Anki to configure card settings.
              </div>
              <div v-else class="form-grid">
                <label class="form-group">
                  <span>Note type</span>
                  <select
                    :value="localSettings.noteType"
                    @change="(e) => onModelChange((e.target as HTMLSelectElement).value)"
                  >
                    <option value="">Select a note type…</option>
                    <option v-for="model in modelNames" :key="model" :value="model">
                      {{ model }}
                    </option>
                  </select>
                </label>

                <template v-if="localSettings.noteType">
                  <label class="form-group">
                    <span>Front field</span>
                    <select
                      :value="localSettings.frontField"
                      @change="
                        (e) => onFieldChange('frontField', (e.target as HTMLSelectElement).value)
                      "
                    >
                      <option value="">Select…</option>
                      <option v-for="field in availableFields" :key="field" :value="field">
                        {{ field }}
                      </option>
                    </select>
                    <small class="field-hint">Used to find the target card</small>
                  </label>

                  <label class="form-group">
                    <span>Sentence field</span>
                    <select
                      :value="localSettings.sentenceField"
                      @change="
                        (e) => onFieldChange('sentenceField', (e.target as HTMLSelectElement).value)
                      "
                    >
                      <option value="">Don't update</option>
                      <option v-for="field in availableFields" :key="field" :value="field">
                        {{ field }}
                      </option>
                    </select>
                  </label>

                  <label class="form-group">
                    <span>Audio field</span>
                    <select
                      :value="localSettings.audioField"
                      @change="
                        (e) => onFieldChange('audioField', (e.target as HTMLSelectElement).value)
                      "
                    >
                      <option value="">Don't update</option>
                      <option v-for="field in availableFields" :key="field" :value="field">
                        {{ field }}
                      </option>
                    </select>
                  </label>

                  <label class="form-group">
                    <span>Image field</span>
                    <select
                      :value="localSettings.imageField"
                      @change="
                        (e) => onFieldChange('imageField', (e.target as HTMLSelectElement).value)
                      "
                    >
                      <option value="">Don't update</option>
                      <option v-for="field in availableFields" :key="field" :value="field">
                        {{ field }}
                      </option>
                    </select>
                  </label>
                </template>
                <div v-if="loadingModels" class="muted-box">Loading note types…</div>
                <div v-else-if="modelsError" class="error-text">{{ modelsError }}</div>
              </div>
            </section>

            <section class="section">
              <div class="section-header">
                <h3>Media</h3>
              </div>
              <div class="form-grid">
                <label class="form-group">
                  <span>Image format</span>
                  <select v-model="localMediaSettings.image.format">
                    <option value="jpg">JPEG</option>
                    <option value="webp">WebP</option>
                    <option value="avif">AVIF</option>
                  </select>
                </label>
                <label class="form-group">
                  <span>Image quality (WebP)</span>
                  <input
                    v-model.number="localMediaSettings.image.quality"
                    type="number"
                    min="0"
                    max="100"
                  />
                  <small class="field-hint">0-100, used for WebP encoding</small>
                </label>
                <label class="form-group">
                  <span>Audio format</span>
                  <select v-model="localMediaSettings.audio.format">
                    <option value="mp3">MP3</option>
                    <option value="opus">Opus</option>
                  </select>
                </label>
                <label class="form-group">
                  <span>Audio bitrate</span>
                  <input
                    v-model="localMediaSettings.audio.bitrate"
                    type="text"
                    placeholder="128k"
                  />
                  <small class="field-hint">Leave blank for defaults (128k / 96k).</small>
                </label>
                <label class="form-group">
                  <span>Audio filters (ffmpeg -af)</span>
                  <input
                    v-model="localMediaSettings.audio.filters"
                    type="text"
                    placeholder="loudnorm=I=-16:TP=-1.5:LRA=11"
                  />
                  <small class="field-hint">Optional, for example loudnorm=…</small>
                </label>
              </div>
            </section>
          </div>

          <footer class="modal-footer">
            <button class="btn ghost" @click="cancelSettings">Cancel</button>
            <button class="btn primary" @click="saveSettings">
              Save
            </button>
          </footer>
        </div>
      </div>
    </Teleport>

    <Teleport to="body">
      <div class="toast-stack">
        <TransitionGroup name="toast">
          <div
            v-for="t in toasts"
            :key="t.id"
            class="toast"
            :data-type="t.type"
            @click="dismissToast(t.id)"
          >
            <span class="icon">{{ toastIcons[t.type] }}</span>
            <span class="message">{{ t.message }}</span>
            <button v-if="t.action" class="btn inline" @click.stop="t.action.onClick">
              {{ t.action.label }}
            </button>
          </div>
        </TransitionGroup>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
  :global(*),
  :global(*::before),
  :global(*::after) {
    box-sizing: border-box;
  }

  :global(body) {
    margin: 0;
    background: #14171c;
    color: #e9edf2;
    font-family:
      'Inter',
      system-ui,
      -apple-system,
      sans-serif;
  }

  :global(a) {
    color: inherit;
  }

  .app {
    min-height: 100vh;
    display: flex;
    flex-direction: column;
  }

  .topbar {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    background: #1b1f26;
    border-bottom: 1px solid #252b34;
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .title {
    font-weight: 700;
    letter-spacing: 0.4px;
  }

  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    text-transform: capitalize;
    color: #a7b4c7;
    font-size: 0.95em;
  }

  .status .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: #555;
  }

  .status[data-state='connected'] .dot {
    background: #3ddc97;
  }

  .status[data-state='connecting'] .dot {
    background: #f4c542;
  }

  .status .port {
    color: #7c8aa1;
  }

  .controls {
    margin-left: auto;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .field {
    display: flex;
    align-items: center;
    gap: 6px;
    color: #9aa5b5;
    font-size: 0.9em;
  }

  .input {
    background: #11151b;
    border: 1px solid #2a303a;
    color: #e9edf2;
    padding: 6px 8px;
    border-radius: 6px;
    min-width: 120px;
  }

  .btn {
    border: 1px solid #2f3742;
    background: #232934;
    color: #e9edf2;
    padding: 8px 12px;
    border-radius: 6px;
    cursor: pointer;
    transition:
      background 0.15s ease,
      border-color 0.15s ease,
      color 0.15s ease;
  }

  .btn:hover {
    background: #2e3643;
  }

  .btn.ghost {
    background: transparent;
  }

  .btn.primary {
    background: #2d5a3d;
    border-color: #3b6f4e;
  }

  .btn.primary:hover {
    background: #38764c;
  }

  .btn.inline {
    padding: 4px 8px;
    font-size: 0.85em;
  }

  .btn.muted {
    opacity: 0.7;
    cursor: wait;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .icon-btn {
    border: none;
    background: none;
    color: #aeb6c5;
    cursor: pointer;
    font-size: 1.4rem;
    padding: 4px;
    border-radius: 4px;
  }

  .icon-btn:hover {
    background: #2a313c;
    color: #fff;
  }

  .icon-btn.close {
    line-height: 1;
  }

  .main {
    flex: 1;
    padding: 16px;
    padding-bottom: calc(var(--selection-bar-height, 72px) + 16px);
  }

  .empty {
    color: #6c7687;
    padding: 0 8px;
    font-size: 1.05em;
  }

  .messages {
    list-style: none;
    padding: 0;
    margin: 0;
  }

  .bottom-anchor {
    scroll-margin-bottom: calc(var(--selection-bar-height, 72px) + 16px);
  }

  .message-row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 14px 0;
    border-bottom: 1px solid #202630;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .message-row:hover {
    background: rgba(255, 255, 255, 0.04);
  }

  .message-row.selected {
    background: rgba(90, 154, 202, 0.15);
    border-left: 3px solid #5a9aca;
    padding-left: 13px;
  }

  .subtitle-text {
    flex: 0 1 auto;
    min-width: 0;
    font-size: 1.1em;
    line-height: 1.6;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .thumb-action {
    position: relative;
    display: flex;
    align-items: center;
  }

  .range-audio-btn {
    margin-left: auto;
    margin-right: 8px;
  }

  .icon-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .icon-btn {
    width: 40px;
    height: 40px;
    padding: 8px;
    background: transparent;
    border: none;
    border-radius: 8px;
    color: #7b8696;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: all 0.15s ease;
  }

  .icon-btn:hover {
    background: rgba(255, 255, 255, 0.08);
    color: #fff;
  }

  .icon-btn.active {
    color: #5a9aca;
  }

  .icon-btn.loading {
    opacity: 0.4;
    cursor: wait;
  }

  .icon-btn svg {
    width: 22px;
    height: 22px;
  }

  .thumb-preview {
    position: absolute;
    top: calc(100% + 8px);
    left: 50%;
    transform: translateX(-50%);
    z-index: 10;
    background: #0f1318;
    border: 1px solid #1f252e;
    border-radius: 8px;
    padding: 8px;
    box-shadow: 0 10px 32px rgba(0, 0, 0, 0.5);
    pointer-events: none;
  }

  .thumb-preview img {
    max-width: 420px;
    max-height: 240px;
    display: block;
    border-radius: 4px;
  }

  .selection-bar {
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    background: rgba(16, 20, 26, 0.96);
    border-top: 1px solid #273041;
    backdrop-filter: blur(8px);
    z-index: 3;
  }

  .selection-bar.inactive {
    opacity: 0.8;
  }

  .selection-left {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: 1;
    min-width: 0;
  }

  .selection-right {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .selection-hint {
    color: #76849a;
    font-size: 0.95em;
    font-style: italic;
  }

  .selection-count {
    font-weight: 600;
  }

  .target-card {
    font-size: 0.95em;
    color: #8ab4d4;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .target-card.loading {
    color: #888;
    font-style: italic;
  }

  .target-card.error {
    color: #c9a054;
  }

  .selection-btn {
    padding: 10px 14px;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    font-size: 0.95em;
    color: #e9edf2;
  }

  .selection-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .selection-btn.send-btn {
    background: #2d5a3d;
  }

  .selection-btn.send-btn:hover {
    background: #38764c;
  }

  .selection-btn.clear-btn {
    background: #343a45;
  }

  .selection-btn.clear-btn:hover {
    background: #3e4552;
  }

  .modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 30;
    padding: 12px;
  }

  .modal {
    background: #12171e;
    border: 1px solid #2a303a;
    border-radius: 10px;
    width: min(540px, 92vw);
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    box-shadow: 0 20px 50px rgba(0, 0, 0, 0.35);
  }

  .modal-header,
  .modal-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 16px;
    border-bottom: 1px solid #1f252e;
  }

  .modal-footer {
    border-top: 1px solid #1f252e;
    border-bottom: none;
  }

  .modal-body {
    padding: 14px 16px 18px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .section {
    background: #0f1318;
    border: 1px solid #1f252d;
    border-radius: 8px;
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .section h3 {
    margin: 0;
    font-size: 1rem;
    color: #cfd7e3;
  }

  .section .subtle {
    color: #7e8898;
    font-size: 0.9em;
  }

  .connection-row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .status-pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    border-radius: 999px;
    background: #1c222c;
    color: #cfd7e3;
    font-size: 0.9em;
  }

  .status-pill.success {
    color: #3ddc97;
    background: rgba(61, 220, 151, 0.12);
  }

  .status-pill.error {
    color: #ef4444;
    background: rgba(239, 68, 68, 0.12);
  }

  .hint {
    margin: 0;
    color: #7e8898;
    font-size: 0.9em;
  }

  .form-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 10px;
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
    color: #cfd7e3;
  }

  .form-group select,
  .form-group input {
    background: #0c0f14;
    border: 1px solid #1f252e;
    color: #e9edf2;
    padding: 8px 10px;
    border-radius: 6px;
  }

  .field-hint {
    color: #7e8898;
    font-size: 0.85em;
  }

  .muted-box {
    background: #0c0f14;
    border: 1px dashed #2a303b;
    color: #7e8898;
    padding: 10px 12px;
    border-radius: 6px;
  }

  .error-text {
    color: #ef4444;
  }

  .toast-stack {
    position: fixed;
    bottom: 20px;
    right: 20px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 60;
    pointer-events: none;
  }

  .toast {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px;
    border-radius: 8px;
    background: #1b2028;
    border: 1px solid #2c343f;
    color: #e9edf2;
    box-shadow: 0 10px 28px rgba(0, 0, 0, 0.4);
    pointer-events: auto;
    cursor: pointer;
  }

  .toast .icon {
    font-size: 1.1em;
  }

  .toast .message {
    font-size: 0.95em;
  }

  .toast[data-type='success'] {
    border-left: 4px solid #3ddc97;
  }

  .toast[data-type='error'] {
    border-left: 4px solid #ef4444;
  }

  .toast[data-type='warning'] {
    border-left: 4px solid #f4c542;
  }

  .toast[data-type='info'] {
    border-left: 4px solid #5a9aca;
  }

  .toast-enter-active {
    transition: all 0.25s ease-out;
  }

  .toast-leave-active {
    transition: all 0.2s ease-in;
  }

  .toast-enter-from,
  .toast-leave-to {
    opacity: 0;
    transform: translateX(40px);
  }

  .toast-move {
    transition: transform 0.2s ease;
  }

  @media (max-width: 640px) {
    .controls {
      width: 100%;
      justify-content: space-between;
    }

    .toast-stack {
      right: 10px;
      left: 10px;
    }

    .toast {
      width: 100%;
    }
  }
</style>
