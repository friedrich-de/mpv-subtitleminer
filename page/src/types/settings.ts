export interface MediaSettings {
  audioOffsetStart: number
  audioOffsetEnd: number
  imageFormat: 'webp' | 'jpeg' | 'avif'
  imageQuality: number
  imageAnimated: boolean
  audioFormat: 'opus' | 'mp3'
  audioQuality: number
  audioFilters: string
  imageSize: string
  imageAdvanced: boolean
  imageAdvancedArgs: string
  imageAdvancedExtension: string
  audioAdvanced: boolean
  audioAdvancedArgs: string
  audioAdvancedExtension: string
  playAudioOnAdd: boolean
}

export interface WordCardSettings {
  noteType: string
  frontField: string
  sentenceField: string
  secondaryField: string
  audioField: string
  imageField: string
  maxCardAgeMinutes: number
  tags: string[]
}

export interface SentenceCardSettings {
  deck: string
  noteType: string
  primaryField: string
  secondaryField: string
  audioField: string
  imageField: string
  metadataField: string
  tags: string[]
}

export interface AnkiSettings {
  word: WordCardSettings
  sentence: SentenceCardSettings
}

export interface ConnectionSettings {
  host: string
  ports: number[]
}

export interface DisplaySettings {
  subtitleFontSize: number
  secondaryFontSize: number
  mediaFilenameRegex: string
  mediaFilenameRegexEnabled: boolean
  sentenceCleanRegex: string
  sentenceCleanRegexEnabled: boolean
  secondaryCleanRegex: string
  secondaryCleanRegexEnabled: boolean
  showSecondaryColumn: boolean
  timelineZoom: number
  primaryColumnFraction: number
}

export interface Settings {
  anki: AnkiSettings
  connection: ConnectionSettings
  media: MediaSettings
  display: DisplaySettings
}
