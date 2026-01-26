<script setup lang="ts">
  import MediaConfiguration from './components/MediaConfiguration.vue'
  import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
  import { useToast } from './composables/useToast'
  import { useWebSocket } from './composables/useWebSocket'
  import * as anki from './services/ankiConnect'
  import { isJsonObject, type JsonObject, type JsonValue } from './types/json'
  import type { AnkiSettings, ConnectionSettings, MediaSettings, Settings } from './types/settings'
  import { preserveHtmlTags } from './utils/htmlUtils'

  const DEFAULT_PORTS = [61777, 61778, 61779, 61780, 61781]

  const { toasts, toast, toastIcons, dismissToast } = useToast()

  const STORAGE_KEY = 'mpv_subtitle_tool_settings'
  const defaultSettings: Settings = {
    anki: { noteType: '', frontField: '', sentenceField: '', audioField: '', imageField: '', maxCardAgeMinutes: 5 },
    connection: { host: '127.0.0.1', ports: [...DEFAULT_PORTS] },
    media: {
      audioOffsetStart: 0.25,
      audioOffsetEnd: 0.25,
      imageFormat: 'jpeg',
      imageQuality: 5,
      imageAnimated: false,
      audioFormat: 'mp3',
      audioQuality: 128,
      audioFilters: '',
      imageSize: '640:-2',
      imageAdvanced: false,
      imageAdvancedArgs: '',
      imageAdvancedExtension: '',
      audioAdvanced: false,
      audioAdvancedArgs: '',
      audioAdvancedExtension: '',
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
          media: { ...defaultSettings.media, ...parsed.media },
        }
      }
    } catch (err) {
      console.warn('Failed to load settings', err)
    }
    return { ...defaultSettings }
  }

  const settings = ref<Settings>(loadSettings())

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
  const localConnection = ref<ConnectionSettings>({ ...settings.value.connection })
  const localMedia = ref<MediaSettings>({ ...settings.value.media })
  const localPortInput = ref('')

  const modelNames = computed(() => Object.keys(modelsWithFields.value).sort())
  const availableFields = computed(() => {
    const model = localSettings.value.noteType
    return model ? (modelsWithFields.value[model] ?? []) : []
  })
  const settingsValid = computed(() => {
    const { noteType, sentenceField, audioField, imageField } = localSettings.value
    // Allow saving if Anki is not configured
    if (!noteType) return true
    return !!sentenceField || !!audioField || !!imageField
  })

  watch(showSettings, (isOpen) => {
    if (isOpen) {
      localSettings.value = { ...settings.value.anki }
      localConnection.value = { ...settings.value.connection }
      localMedia.value = { ...settings.value.media }
      localPortInput.value = localConnection.value.ports.join(', ')
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
      maxCardAgeMinutes: 5,
    }
  }

  function onFieldChange(field: keyof AnkiSettings, value: string | number) {
    // @ts-ignore - dynamic assignment
    localSettings.value = { ...localSettings.value, [field]: value }
  }

  function saveSettings() {
    const mediaSettingsChanged =
      localMedia.value.audioOffsetStart !== settings.value.media.audioOffsetStart ||
      localMedia.value.audioOffsetEnd !== settings.value.media.audioOffsetEnd ||
      localMedia.value.imageFormat !== settings.value.media.imageFormat ||
      localMedia.value.imageQuality !== settings.value.media.imageQuality ||
      localMedia.value.imageAnimated !== settings.value.media.imageAnimated ||
      localMedia.value.audioFormat !== settings.value.media.audioFormat ||
      localMedia.value.audioQuality !== settings.value.media.audioQuality ||
      localMedia.value.audioFilters !== settings.value.media.audioFilters ||
      localMedia.value.imageSize !== settings.value.media.imageSize ||
      localMedia.value.imageAdvanced !== settings.value.media.imageAdvanced ||
      localMedia.value.imageAdvancedArgs !== settings.value.media.imageAdvancedArgs ||
      localMedia.value.audioAdvanced !== settings.value.media.audioAdvanced ||
      localMedia.value.audioAdvancedArgs !== settings.value.media.audioAdvancedArgs

    if (localMedia.value.imageAdvanced) {
      if (!localMedia.value.imageAdvancedExtension) {
        localMedia.value.imageAnimated = false
      }
    } else if (localMedia.value.imageFormat !== 'avif' && localMedia.value.imageFormat !== 'webp') {
      localMedia.value.imageAnimated = false
    }

    settings.value.anki = { ...localSettings.value }
    settings.value.connection = { ...localConnection.value }
    settings.value.media = { ...localMedia.value }
    
    if (mediaSettingsChanged) {
      for (const msg of messages.value) {
        msg.audio = undefined
        msg.thumbnail = undefined
      }
    }

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
    audio?: string
    sourcePort: number
    uid: string
  }

  const messages = ref<SubtitleMessage[]>([])
  const bottomRef = ref<HTMLElement | null>(null)
  const hoveredThumbnailUid = ref<string | null>(null)
  const loadingMedia = ref<Record<string, boolean>>({})
  const selectedMessages = ref<Set<string>>(new Set())
  const currentAudio = ref<HTMLAudioElement | null>(null)
  const pendingAudioRange = ref<{
    startId: number
    endId: number
    data: string
    port: number
  } | null>(null)
  const selectionBarRef = ref<HTMLElement | null>(null)
  const selectionBarHeight = ref(0)
  let selectionBarObserver: ResizeObserver | null = null
  const sendingToAnki = ref<Record<string, boolean>>({})
  const ankiSuccess = ref<Record<string, boolean>>({})
  const ankiError = ref<Record<string, string>>({})
  const targetCardPreview = ref<string | null>(null)
  const loadingTargetCard = ref(false)

  const host = computed(() => settings.value.connection.host)
  const ports = computed(() => settings.value.connection.ports)

  function updateLocalPorts(raw: string) {
    localPortInput.value = raw
    const parsed = raw
      .split(/[\s,]+/)
      .map((v) => parseInt(v, 10))
      .filter((n) => Number.isInteger(n) && n > 0 && n <= 65535)
    if (parsed.length) {
      localConnection.value.ports = parsed
    }
  }

  function resetLocalConnectionDefaults() {
    localConnection.value.host = defaultSettings.connection.host
    localConnection.value.ports = [...DEFAULT_PORTS]
    localPortInput.value = localConnection.value.ports.join(', ')
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
        const media = parseMediaMessage(d)
        if (!media) return

        const msg = messages.value.find((m) => m.id === media.id && m.sourcePort === port)
        if (msg) {
          if (type === 'thumbnail') {
            msg.thumbnail = media.data
          } else {
            msg.audio = media.data
            playAudio(media.data)
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

  function parseMediaMessage(d: JsonObject): { id: number; data: string } | null {
    const id = asNumber(d.id)
    const data = asString(d.data)
    if (id === null || data === null) return null
    return { id, data }
  }

  function parseAudioRangeMessage(
    d: JsonObject,
  ): { startId: number; endId: number; data: string } | null {
    const startId = asNumber(d.start_id)
    const endId = asNumber(d.end_id)
    const data = asString(d.data)
    if (startId === null || endId === null || data === null) return null
    return { startId, endId, data }
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

  const getAudioParams = () => {
    const media = showSettings.value ? localMedia.value : settings.value.media
    return {
      offset_start: media.audioOffsetStart,
      offset_end: media.audioOffsetEnd,
      audio_config: {
        format: media.audioAdvanced ? media.audioAdvancedExtension : media.audioFormat,
        quality: media.audioQuality,
        filters: media.audioFilters,
        advanced_args: media.audioAdvanced
          ? media.audioAdvancedArgs
          : null,
      },
    }
  }

  const getImageParams = () => {
    const media = showSettings.value ? localMedia.value : settings.value.media
    return {
      image_config: {
        format: media.imageAdvanced ? media.imageAdvancedExtension : media.imageFormat,
        quality: media.imageQuality,
        is_animated: media.imageAnimated,
        size: media.imageSize,
        advanced_args: media.imageAdvanced
          ? media.imageAdvancedArgs
          : null,
      },
    }
  }

  const sendToPort = (payload: JsonValue, port: number | undefined): boolean => {
    if (!port) return false
    return ws.send(payload, port)
  }

  const requestThumbnail = (msg: SubtitleMessage) => {
    if (ws.status.value !== 'connected' || msg.thumbnail) return
    const key = `thumb-${msg.uid}`
    if (loadingMedia.value[key]) return
    const params = getImageParams()
    const payload = { request: 'thumbnail', id: msg.id, ...params }
    if (!sendToPort(payload, msg.sourcePort)) {
      toast.error(`Not connected to port ${msg.sourcePort}`)
      return
    }
    loadingMedia.value[key] = true
  }

  const requestAudio = (msg: SubtitleMessage) => {
    if (ws.status.value !== 'connected') return
    if (msg.audio) {
      playAudio(msg.audio)
      return
    }
    const key = `audio-${msg.uid}`
    if (loadingMedia.value[key]) return
    const payload: Record<string, JsonValue> = {
      request: 'audio',
      id: msg.id,
      ...getAudioParams(),
    }
    if (!sendToPort(payload, msg.sourcePort)) {
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
      playAudio(audioData)
    }
  }

  const playAudio = (audioBase64: string) => {
    if (currentAudio.value) {
      currentAudio.value.pause()
      currentAudio.value = null
    }
    let mimeType = 'audio/ogg; codecs=opus'
    if (settings.value.media.audioFormat === 'mp3') {
      mimeType = 'audio/mpeg'
    }
    const audio = new Audio(`data:${mimeType};base64,${audioBase64}`)
    currentAudio.value = audio
    audio.addEventListener('ended', () => {
      if (currentAudio.value === audio) {
        currentAudio.value = null
      }
    })
    void audio.play()
  }

  const generateMediaFilename = (msgId: number, type: 'audio' | 'image') => {
    const timestamp = Date.now()
    const media = showSettings.value ? localMedia.value : settings.value.media
    let ext = 'webp'

    if (type === 'audio') {
      if (media.audioAdvanced) {
        ext = media.audioAdvancedExtension || 'mp3'
      } else {
        ext = media.audioFormat === 'mp3' ? 'mp3' : 'opus'
      }
    } else {
      if (media.imageAdvanced) {
        ext = media.imageAdvancedExtension || 'jpg'
      } else {
        const fmt = media.imageFormat
        ext = fmt === 'jpeg' ? 'jpg' : fmt
      }
    }

    if (ext.toLowerCase() === 'jpeg') ext = 'jpg'
    ext = ext.replace(/^\.+/, '')

    return `mpv_subtitleminer_${msgId}_${timestamp}.${ext.toLowerCase()}`
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

      const maxAgeMinutes = settings.value.anki.maxCardAgeMinutes ?? 5
      if (maxAgeMinutes > 0) {
        const thresholdMs = maxAgeMinutes * 60000

        if (Date.now() - targetNote.noteId > thresholdMs) {
          throw new Error(`Cannot add to card: The latest card is too old (> ${maxAgeMinutes} minutes).`)
        }
      }

      const fieldUpdates: Record<string, string> = {}

      if (sentenceField) {
        const text = selectedMsgs.map((m) => m.subtitle).join(' ')
        const existingSentence = targetNote.fields[sentenceField]?.value ?? ''
        fieldUpdates[sentenceField] = preserveHtmlTags(existingSentence, text)
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
                : first.audio || (await requestMediaFromServer(first, 'audio'))

        if (audioData) {
          const filename = generateMediaFilename(primaryId, 'audio')
          await anki.storeMediaFile(filename, audioData)
          fieldUpdates[audioField] = `[sound:${filename}]`
        }
      }

      if (imageField) {
        let imageData = (selectedMsgs.length === 1) ? first.thumbnail : undefined

        if (!imageData) {
          imageData = await requestMediaFromServer(
            first, 
            'thumbnail', 
            selectedMsgs.length > 1 ? last.id : undefined
          )
        }
        if (imageData) {
          const filename = generateMediaFilename(primaryId, 'image')
          await anki.storeMediaFile(filename, imageData)
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
    endId?: number,
  ): Promise<string | undefined> => {
    return new Promise((resolve) => {
      if (ws.status.value !== 'connected') {
        resolve(undefined)
        return
      }

      const key = `${type === 'thumbnail' ? 'thumb' : 'audio'}-${msg.uid}${endId ? `-${endId}` : ''}`
      if (loadingMedia.value[key]) {
        const checkInterval = setInterval(() => {
          if (type === 'thumbnail' && msg.thumbnail) {
            clearInterval(checkInterval)
            resolve(msg.thumbnail)
          } else if (type === 'audio' && msg.audio) {
            clearInterval(checkInterval)
            resolve(msg.audio)
          } else if (!loadingMedia.value[key]) {
            clearInterval(checkInterval)
            resolve(undefined)
          }
        }, 100)

        setTimeout(() => {
          clearInterval(checkInterval)
          resolve(type === 'thumbnail' ? msg.thumbnail : msg.audio)
        }, 10000)
        return
      }

      loadingMedia.value[key] = true
      const payload: Record<string, JsonValue> = {
        request: type,
        id: msg.id,
        ...(endId ? { end_id: endId } : {}),
        ...(type === 'thumbnail' ? getImageParams() : getAudioParams()),
      }
      
      if (!sendToPort(payload, msg.sourcePort)) {
        delete loadingMedia.value[key]
        resolve(undefined)
        return
      }

      const checkInterval = setInterval(() => {
        if (type === 'thumbnail' && msg.thumbnail) {
          clearInterval(checkInterval)
          resolve(msg.thumbnail)
        } else if (type === 'audio' && msg.audio) {
          clearInterval(checkInterval)
          resolve(msg.audio)
        }
      }, 100)

      setTimeout(() => {
        clearInterval(checkInterval)
        delete loadingMedia.value[key]
        resolve(type === 'thumbnail' ? msg.thumbnail : msg.audio)
      }, 10000)
    })
  }

  const requestAudioRange = (
    startId: number,
    endId: number,
    port: number,
  ): Promise<string | undefined> => {
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
            resolve(result.data)
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
      if (
        !sendToPort(
          {
            request: 'audio_range',
            start_id: startId,
            end_id: endId,
            ...getAudioParams(),
          },
          port,
        )
      ) {
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
          resolve(result.data)
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
        <button class="btn" type="button" @click="ws.connect">Connect</button>
        <button class="btn ghost" type="button" @click="ws.disconnect">Disconnect</button>
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
                <img :src="`data:image/${settings.media.imageFormat};base64,${message.thumbnail}`" alt="Thumbnail" />
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
                <h3>MPV Connection</h3>
                <button class="btn ghost inline" @click="resetLocalConnectionDefaults">
                  Reset to defaults
                </button>
              </div>
              <div class="form-grid">
                <label class="form-group">
                  <span>Host IP</span>
                  <input
                    v-model="localConnection.host"
                    type="text"
                    placeholder="127.0.0.1"
                  />
                </label>
                <label class="form-group">
                  <span>Ports (comma separated)</span>
                  <input
                    :value="localPortInput"
                    type="text"
                    placeholder="61777, 61778"
                    @input="(e) => updateLocalPorts((e.target as HTMLInputElement).value)"
                  />
                </label>
              </div>
            </section>

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

                  <label class="form-group">
                    <span>Max card age (minutes)</span>
                    <input
                      type="number"
                      min="0"
                      step="0.1"
                      :value="localSettings.maxCardAgeMinutes"
                      @input="(e) => onFieldChange('maxCardAgeMinutes', parseFloat((e.target as HTMLInputElement).value) || 0)"
                    />
                    <small class="field-hint">Prevent adding to cards older than this (0 for no limit).</small>
                  </label>
                </template>
                <div v-if="loadingModels" class="muted-box">Loading note types…</div>
                <div v-else-if="modelsError" class="error-text">{{ modelsError }}</div>
              </div>
            </section>

            <section class="section">
              <div class="section-header">
                <h3>Media configuration</h3>
              </div>
              <MediaConfiguration 
                v-model="localMedia" 
                :default-settings="defaultSettings"
              />
            </section>
          </div>

          <footer class="modal-footer">
            <button class="btn ghost" @click="cancelSettings">Cancel</button>
            <button class="btn primary" :disabled="!settingsValid" @click="saveSettings">
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

  .input-group {
    display: flex;
    align-items: center;
    position: relative;
  }

  .input-group input,
  .input-group select {
    width: 100%;
    padding-right: 32px;
    -moz-appearance: textfield;
    appearance: none;
  }

  .input-group input::-webkit-outer-spin-button,
  .input-group input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    appearance: none;
    margin: 0;
  }

  .btn-reset {
    position: absolute;
    right: 4px;
    background: none;
    border: none;
    color: #6c7687;
    cursor: pointer;
    padding: 2px;
    border-radius: 4px;
    font-size: 0.9em;
    line-height: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    pointer-events: none;
    transition: opacity 0.2s ease, background 0.15s ease;
  }

  .btn-reset.visible {
    opacity: 1;
    pointer-events: auto;
  }

  .btn-reset:hover {
    color: #e9edf2;
    background: #2a313c;
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
  .field-hint.full-width {
    display: block;
    margin-top: 0.5rem;
    text-align: left;
  }
</style>
