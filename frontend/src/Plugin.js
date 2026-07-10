import React, { useEffect, useRef, useState } from 'react'
import {
  api,
  useAlert,
  Page,
  ListHeader,
  Card,
  SectionHeader,
  StatTile,
  KeyVal,
  StatusDot,
  Toggle,
  TextField,
  ModalConfirm,
  Loading,
  EmptyState,
  Badge,
  BadgeText,
  Box,
  Button,
  ButtonText,
  HStack,
  Text,
  VStack
} from '@spr-networks/plugin-ui'

const BASE = `/plugins/${api.pluginURI() || 'spr-nostr'}`

const MODES = [
  { key: 'both', label: 'Read & write', help: 'Clients can publish events and read them back. Recommended.' },
  { key: 'read', label: 'Read-only', help: 'Clients can read stored events; publishing (EVENT) is rejected.' },
  { key: 'write', label: 'Write-only', help: 'Clients can publish events; reading (REQ) is rejected.' }
]

const modeLabel = (m) => (MODES.find((x) => x.key === m) || {}).label || m || '—'

const fmtUptime = (secs) => {
  if (!secs || secs < 0) return '—'
  const d = Math.floor(secs / 86400)
  const h = Math.floor((secs % 86400) / 3600)
  const m = Math.floor((secs % 3600) / 60)
  if (d) return `${d}d ${h}h`
  if (h) return `${h}h ${m}m`
  if (m) return `${m}m`
  return `${Math.floor(secs)}s`
}

const fmtBytes = (n) => {
  if (n === null || n === undefined) return '—'
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = n / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`
}

const validatePort = (portStr) => {
  if (!String(portStr).trim()) return 'Port is required'
  const n = Number(portStr)
  if (!Number.isInteger(n) || n < 1 || n > 65535)
    return 'Enter a whole number between 1 and 65535'
  return ''
}

const CopyButton = ({ value, label = 'Copy', onCopied, onFailed }) => (
  <Button
    size="xs"
    variant="outline"
    isDisabled={!value}
    onPress={() => {
      if (value && navigator?.clipboard?.writeText) {
        navigator.clipboard.writeText(value).then(onCopied).catch(onFailed)
      } else {
        onFailed()
      }
    }}
  >
    <ButtonText>{label}</ButtonText>
  </Button>
)

// Mono, bordered address box + copy button — the one thing a user pastes.
const AddressBox = ({ value, onCopied, onFailed }) => (
  <HStack space="sm" alignItems="center" flexWrap="wrap">
    <Box
      flex={1}
      minWidth={260}
      px="$3"
      py="$2.5"
      borderRadius="$lg"
      borderWidth={1}
      borderColor="$muted200"
      bg="$backgroundContentLight"
      sx={{ _dark: { bg: '$backgroundContentDark', borderColor: '$muted700' } }}
    >
      <Text
        size="sm"
        selectable
        sx={{ '@base': { fontFamily: 'monospace', wordBreak: 'break-all' } }}
      >
        {value || '—'}
      </Text>
    </Box>
    <CopyButton value={value} label="Copy address" onCopied={onCopied} onFailed={onFailed} />
  </HStack>
)

export default function Plugin() {
  const alert = useAlert()
  const [loading, setLoading] = useState(true)
  const [unreachable, setUnreachable] = useState(false)
  const [status, setStatus] = useState(null)
  const [config, setConfig] = useState(null)

  // settings form
  const [portStr, setPortStr] = useState('')
  const [mode, setMode] = useState('both')
  const [requireAuth, setRequireAuth] = useState(false)
  const [saving, setSaving] = useState(false)

  // deliberate actions
  const [showRestart, setShowRestart] = useState(false)
  const [restarting, setRestarting] = useState(false)
  const [showSetup, setShowSetup] = useState(false)

  const formInit = useRef(false)

  const refresh = () => {
    return Promise.allSettled([api.get(`${BASE}/status`), api.get(`${BASE}/config`)]).then(
      ([s, c]) => {
        if (s.status === 'fulfilled') {
          setStatus(s.value)
          setUnreachable(false)
        } else {
          setUnreachable(true)
        }
        if (c.status === 'fulfilled' && c.value) {
          setConfig(c.value)
          if (!formInit.current) {
            setPortStr(String(c.value.Port))
            setMode(c.value.Mode || 'both')
            setRequireAuth(!!c.value.RequireAuth)
            formInit.current = true
          }
        }
        setLoading(false)
      }
    )
  }

  useEffect(() => {
    refresh()
    const t = setInterval(refresh, 10000)
    return () => clearInterval(t)
  }, [])

  const portError = validatePort(portStr)
  const dirty =
    !!config &&
    (portStr !== String(config.Port) ||
      mode !== (config.Mode || 'both') ||
      requireAuth !== !!config.RequireAuth)

  const resetForm = () => {
    if (!config) return
    setPortStr(String(config.Port))
    setMode(config.Mode || 'both')
    setRequireAuth(!!config.RequireAuth)
  }

  const save = () => {
    if (portError || !dirty || saving) return
    setSaving(true)
    api
      .put(`${BASE}/config`, {
        Port: Number(portStr),
        Mode: mode,
        RequireAuth: requireAuth
      })
      .then((c) => {
        setConfig(c)
        alert.success('Saved — relay restarted')
        refresh()
      })
      .catch((err) => alert.error('Failed to save settings', err))
      .finally(() => setSaving(false))
  }

  const restart = () => {
    setRestarting(true)
    api
      .post(`${BASE}/restart`)
      .then(() => {
        alert.success('Relay restarted')
        refresh()
      })
      .catch((err) => alert.error('Restart failed', err))
      .finally(() => setRestarting(false))
  }

  const copied = () => alert.success('Copied to clipboard')
  const copyFailed = () => alert.warning('Copy failed — select the text and copy manually')

  if (loading) {
    return (
      <Page>
        <Loading />
      </Page>
    )
  }

  if (unreachable || !status) {
    return (
      <Page>
        <ListHeader title="Nostr relay" description="Self-hosted Nostr relay on your router" />
        <Card>
          <EmptyState
            title="Backend unreachable"
            description="The spr-nostr plugin API did not respond. If the plugin was just installed or updated, the container may still be starting."
          >
            <Button
              size="sm"
              onPress={() => {
                setLoading(true)
                refresh()
              }}
            >
              <ButtonText>Retry</ButtonText>
            </Button>
          </EmptyState>
        </Card>
      </Page>
    )
  }

  const running = !!status.Running
  const address = status.Address || ''

  return (
    <Page>
      <ListHeader
        title="Nostr relay"
        description="Self-hosted Nostr relay on your router"
        status={running ? 'Running' : 'Stopped'}
        statusAction={running ? 'success' : 'muted'}
      >
        <Button
          size="sm"
          variant="outline"
          isDisabled={restarting}
          onPress={() => setShowRestart(true)}
        >
          <ButtonText>{restarting ? 'Restarting…' : 'Restart'}</ButtonText>
        </Button>
      </ListHeader>

      {/* ---- hero: state + the copyable relay address ---- */}
      <Card>
        <SectionHeader title="Your relay address" right={<StatusDot online={running} />} />
        <VStack space="md">
          <AddressBox value={address} onCopied={copied} onFailed={copyFailed} />
          <Text size="sm" color="$muted500">
            In your Nostr client, open <Text size="sm" bold>Relays</Text> (or Settings → Network),
            add this URL, and enable read and write. Devices in the SPR{' '}
            <Text size="sm" sx={{ '@base': { fontFamily: 'monospace' } }}>nostr</Text> group reach
            the relay while on your LAN; see the README to expose it to the internet with an SPR
            port forward.
          </Text>
          <HStack>
            <Button size="xs" variant="link" onPress={() => setShowSetup((v) => !v)}>
              <ButtonText>{showSetup ? 'Hide setup steps' : 'Show setup steps'}</ButtonText>
            </Button>
          </HStack>
          {showSetup ? (
            <VStack space="xs">
              <Text size="sm" color="$muted500">
                1. Copy the relay address above.
              </Text>
              <Text size="sm" color="$muted500">
                2. In your Nostr app (Damus, Amethyst, snort, …) open Relays and paste it.
              </Text>
              <Text size="sm" color="$muted500">
                3. Enable read + write, then publish a note — it is stored on your router, not a
                public relay.
              </Text>
            </VStack>
          ) : null}
        </VStack>
      </Card>

      {/* ---- operational numbers ---- */}
      <Card>
        <SectionHeader title="Status" />
        <HStack flexWrap="wrap" gap="$2">
          <StatTile label="State" value={running ? 'Running' : 'Stopped'} />
          <StatTile label="Uptime" value={running ? fmtUptime(status.UptimeSeconds) : '—'} />
          <StatTile label="Mode" value={modeLabel(status.Mode)} />
          <StatTile label="Port" value={String(status.Port || '—')} mono />
          <StatTile label="Version" value={status.Version || '—'} mono />
          <StatTile label="Database" value={fmtBytes(status.DbBytes)} mono />
        </HStack>
        <VStack space="xs" mt="$3">
          <HStack space="md" alignItems="center" flexWrap="wrap">
            <Box flex={1} minWidth={240}>
              <KeyVal label="Listening on" value={status.Host ? `${status.Host}:${status.Port}` : '—'} mono />
            </Box>
            <CopyButton value={address} onCopied={copied} onFailed={copyFailed} />
          </HStack>
          <KeyVal
            label="Client authentication"
            value={status.RequireAuth ? 'Required (NIP-42)' : 'Open'}
          />
          <KeyVal label="Relay engine" value={status.Engine || '—'} mono />
        </VStack>
      </Card>

      {/* ---- settings ---- */}
      <Card>
        <SectionHeader
          title="Settings"
          right={
            status.RequireAuth ? (
              <Badge action="success" variant="outline" borderRadius="$full">
                <BadgeText>Auth required</BadgeText>
              </Badge>
            ) : (
              <Badge action="muted" variant="outline" borderRadius="$full">
                <BadgeText>Open access</BadgeText>
              </Badge>
            )
          }
        />
        <VStack space="md">
          <TextField
            label="Port"
            value={portStr}
            onChangeText={setPortStr}
            placeholder="7777"
            keyboardType="numeric"
            error={portError && dirty ? portError : ''}
            helper="TCP port the relay listens on, on the spr-nostr bridge (container IP)."
          />

          <VStack space="xs">
            <Text>Access mode</Text>
            <HStack space="sm" flexWrap="wrap">
              {MODES.map((m) => (
                <Button
                  key={m.key}
                  size="sm"
                  variant={mode === m.key ? 'solid' : 'outline'}
                  action={mode === m.key ? 'primary' : 'secondary'}
                  onPress={() => setMode(m.key)}
                >
                  <ButtonText>{m.label}</ButtonText>
                </Button>
              ))}
            </HStack>
            <Text size="xs" color="$muted500">
              {(MODES.find((m) => m.key === mode) || {}).help}
            </Text>
          </VStack>

          <HStack justifyContent="space-between" alignItems="center">
            <VStack flex={1} pr="$4">
              <Text>Require client authentication (NIP-42)</Text>
              <Text size="xs" color="$muted500">
                Clients must authenticate with their key before reading or writing. Leave off for an
                open personal relay.
              </Text>
            </VStack>
            <Toggle
              value={requireAuth}
              onPress={() => setRequireAuth(!requireAuth)}
              label="Require client authentication (NIP-42)"
            />
          </HStack>

          <Text size="xs" color="$muted500">
            Note: this relay build ({status.Engine || 'nostr-relay-builder'}) does not serve a
            NIP-11 relay information document, so relay name/description/contact metadata is not
            configurable here.
          </Text>

          <HStack justifyContent="space-between" alignItems="center" flexWrap="wrap" gap="$2">
            <Text size="xs" color="$muted500" flex={1} minWidth={200}>
              Applying restarts the relay; connected clients reconnect automatically.
            </Text>
            <HStack space="sm">
              {dirty ? (
                <Button size="sm" variant="outline" action="secondary" onPress={resetForm}>
                  <ButtonText>Discard</ButtonText>
                </Button>
              ) : null}
              <Button size="sm" isDisabled={!dirty || saving || !!portError} onPress={save}>
                <ButtonText>{saving ? 'Saving…' : 'Save & apply'}</ButtonText>
              </Button>
            </HStack>
          </HStack>
        </VStack>
      </Card>

      <ModalConfirm
        isOpen={showRestart}
        onClose={() => setShowRestart(false)}
        onConfirm={restart}
        title="Restart the Nostr relay?"
        message="Connected clients disconnect briefly and reconnect automatically. Stored events are preserved (they live in the plugin's LMDB database)."
        confirmText="Restart"
      />
    </Page>
  )
}
