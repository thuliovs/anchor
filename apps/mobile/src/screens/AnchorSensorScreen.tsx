import React from 'react';
import {
  Pressable,
  ScrollView,
  StatusBar,
  StyleSheet,
  Text,
  useColorScheme,
  View,
} from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';

import {
  MotionCaptureController,
  type MotionCaptureSnapshot,
  type MotionCaptureStatus,
} from '../motion/MotionCaptureController';

const POSITIONING_INSTRUCTION = 'Posicione o telefone deitado, tela para cima, em retrato, com a borda superior apontando para a frente do veiculo.';
const LOCAL_WARNING = 'Diagnostico local - nenhum dado esta sendo enviado';

export function AnchorSensorScreen({
  controller: providedController,
}: {
  controller?: MotionCaptureController;
}): React.JSX.Element {
  const colorScheme = useColorScheme();
  const palette = colorScheme === 'dark' ? darkPalette : lightPalette;
  const controllerRef = React.useRef<MotionCaptureController | null>(null);

  if (!controllerRef.current) {
    controllerRef.current = providedController ?? new MotionCaptureController();
  }

  const controller = controllerRef.current;
  const [snapshot, setSnapshot] = React.useState<MotionCaptureSnapshot>(controller.getSnapshot());

  React.useEffect(() => {
    const unsubscribe = controller.subscribe(setSnapshot);
    controller.initialize().catch(() => undefined);

    return () => {
      unsubscribe();
      controller.dispose();
    };
  }, [controller]);

  const isCapturing = snapshot.status === 'starting'
    || snapshot.status === 'active'
    || snapshot.status === 'stale';
  const isCheckingSensors = snapshot.status === 'checking_sensors';
  const isBusy = isCheckingSensors || snapshot.status === 'starting';
  const isPrimaryDisabled = snapshot.status === 'unsupported'
    || (!isCapturing && isCheckingSensors);

  function handlePrimaryAction() {
    if (isCapturing) {
      controller.stopCapture('explicit');
      return;
    }

    controller.startCapture().catch(() => undefined);
  }

  return (
    <SafeAreaView style={[styles.safeArea, { backgroundColor: palette.background }]}>
      <StatusBar barStyle={colorScheme === 'dark' ? 'light-content' : 'dark-content'} />
      <View style={styles.root}>
        <ScrollView
          contentContainerStyle={styles.scrollContent}
          alwaysBounceVertical={false}
        >
          <Text style={[styles.title, { color: palette.text }]}>Anchor Sensor</Text>
          <InfoCard palette={palette} title="Estado">
            <DataRow label="Status" value={statusLabel(snapshot.status)} palette={palette} />
            <DataRow
              label="Disponibilidade"
              value={availabilitySummary(snapshot)}
              palette={palette}
            />
          </InfoCard>

          <InfoCard palette={palette} title="Posicionamento obrigatorio">
            <Text style={[styles.bodyText, { color: palette.textMuted }]}>
              {POSITIONING_INSTRUCTION}
            </Text>
            <Text style={[styles.bodyText, styles.spacingTop8, { color: palette.textMuted }]}>
              X+ = direita do veiculo, Y+ = frente do veiculo, Z+ = cima.
            </Text>
          </InfoCard>

          <InfoCard palette={palette} title="Sessao">
            <DataRow label="Session" value={snapshot.sessionId ?? '--'} palette={palette} />
            <DataRow label="Sequence" value={formatInteger(snapshot.sequence)} palette={palette} />
            <DataRow
              label="Tempo monotonic"
              value={formatMicros(snapshot.sessionElapsedUs)}
              palette={palette}
            />
            <DataRow
              label="Taxa observada"
              value={`${snapshot.observedRateHz.toFixed(1)} Hz`}
              palette={palette}
            />
            <DataRow label="Aceitas" value={String(snapshot.acceptedCount)} palette={palette} />
            <DataRow label="Rejeitadas" value={String(snapshot.rejectedCount)} palette={palette} />
            <DataRow
              label="Idade da ultima amostra"
              value={formatAge(snapshot.lastSampleAgeMs)}
              palette={palette}
            />
          </InfoCard>

          <VectorCard
            palette={palette}
            title="Aceleracao linear (m/s²)"
            vector={snapshot.lastSample?.linearAccelerationMps2 ?? null}
          />
          <VectorCard
            palette={palette}
            title="Gravidade (m/s²)"
            vector={snapshot.lastSample?.gravityMps2 ?? null}
          />
          <VectorCard
            palette={palette}
            title="Velocidade angular (rad/s)"
            vector={snapshot.lastSample?.angularVelocityRadS ?? null}
          />

          <InfoCard palette={palette} title="Aviso">
            <Text style={[styles.warningText, { color: palette.warning }]}>{LOCAL_WARNING}</Text>
            {snapshot.errorMessage ? (
              <Text style={[styles.bodyText, styles.spacingTop8, { color: palette.error }]}>
                {snapshot.errorMessage}
              </Text>
            ) : null}
          </InfoCard>
        </ScrollView>

        <View style={[styles.footer, { backgroundColor: palette.background, borderTopColor: palette.border }]}> 
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={isCapturing ? 'Parar captura de sensores' : 'Iniciar captura de sensores'}
            accessibilityState={{
              disabled: isPrimaryDisabled,
              busy: isBusy,
            }}
            android_ripple={{ color: palette.ripple }}
            disabled={isPrimaryDisabled}
            onPress={handlePrimaryAction}
            style={({ pressed }) => [
              styles.primaryButton,
              {
                backgroundColor: isPrimaryDisabled
                  ? palette.buttonDisabled
                  : palette.button,
                opacity: pressed ? 0.92 : 1,
              },
            ]}
          >
            <Text style={[styles.primaryButtonText, { color: palette.buttonText }]}>
              {isCapturing ? 'Parar captura' : 'Iniciar captura'}
            </Text>
          </Pressable>
        </View>
      </View>
    </SafeAreaView>
  );
}

function InfoCard({
  children,
  palette,
  title,
}: React.PropsWithChildren<{
  palette: Palette;
  title: string;
}>): React.JSX.Element {
  return (
    <View style={[styles.card, { backgroundColor: palette.card, borderColor: palette.border }]}> 
      <Text style={[styles.cardTitle, { color: palette.text }]}>{title}</Text>
      {children}
    </View>
  );
}

function VectorCard({
  palette,
  title,
  vector,
}: {
  palette: Palette;
  title: string;
  vector: { x: number; y: number; z: number } | null;
}): React.JSX.Element {
  return (
    <InfoCard palette={palette} title={title}>
      <DataRow label="X" value={formatNumber(vector?.x)} palette={palette} />
      <DataRow label="Y" value={formatNumber(vector?.y)} palette={palette} />
      <DataRow label="Z" value={formatNumber(vector?.z)} palette={palette} />
    </InfoCard>
  );
}

function DataRow({
  label,
  value,
  palette,
}: {
  label: string;
  value: string;
  palette: Palette;
}): React.JSX.Element {
  return (
    <View style={styles.row}>
      <Text style={[styles.rowLabel, { color: palette.textMuted }]}>{label}</Text>
      <Text style={[styles.rowValue, { color: palette.text }]}>{value}</Text>
    </View>
  );
}

function statusLabel(status: MotionCaptureStatus): string {
  switch (status) {
    case 'checking_sensors':
      return 'verificando sensores';
    case 'ready':
      return 'pronto';
    case 'starting':
      return 'iniciando';
    case 'active':
      return 'ativo';
    case 'stale':
      return 'sinal desatualizado';
    case 'stopped':
      return 'parado';
    case 'unsupported':
      return 'nao suportado';
    case 'error':
      return 'erro';
  }
}

function availabilitySummary(snapshot: MotionCaptureSnapshot): string {
  if (!snapshot.availability) {
    return '--';
  }

  const availability = snapshot.availability;
  return [
    `linear: ${availability.linearAcceleration ? 'sim' : 'nao'}`,
    `gravity: ${availability.gravity ? 'sim' : 'nao'}`,
    `gyro: ${availability.gyroscope ? 'sim' : 'nao'}`,
  ].join(' | ');
}

function formatNumber(value: number | undefined): string {
  return typeof value === 'number' ? value.toFixed(4) : '--';
}

function formatInteger(value: number | null): string {
  return typeof value === 'number' ? String(value) : '--';
}

function formatMicros(value: number | null): string {
  return typeof value === 'number' ? `${value} us` : '--';
}

function formatAge(value: number | null): string {
  return typeof value === 'number' ? `${value} ms` : '--';
}

type Palette = {
  background: string;
  card: string;
  border: string;
  text: string;
  textMuted: string;
  warning: string;
  error: string;
  button: string;
  buttonText: string;
  buttonDisabled: string;
  ripple: string;
};

const lightPalette: Palette = {
  background: '#f4f5f7',
  card: '#ffffff',
  border: '#d7dce3',
  text: '#101828',
  textMuted: '#475467',
  warning: '#b54708',
  error: '#b42318',
  button: '#155eef',
  buttonText: '#ffffff',
  buttonDisabled: '#98a2b3',
  ripple: 'rgba(255,255,255,0.2)',
};

const darkPalette: Palette = {
  background: '#0b1220',
  card: '#111827',
  border: '#1f2937',
  text: '#f8fafc',
  textMuted: '#cbd5e1',
  warning: '#fdba74',
  error: '#fda29b',
  button: '#3b82f6',
  buttonText: '#eff6ff',
  buttonDisabled: '#475467',
  ripple: 'rgba(255,255,255,0.18)',
};

const styles = StyleSheet.create({
  safeArea: {
    flex: 1,
  },
  root: {
    flex: 1,
  },
  scrollContent: {
    paddingHorizontal: 16,
    paddingTop: 16,
    paddingBottom: 24,
    gap: 12,
  },
  title: {
    fontSize: 28,
    fontWeight: '700',
  },
  card: {
    borderWidth: 1,
    borderRadius: 18,
    padding: 16,
  },
  cardTitle: {
    fontSize: 18,
    fontWeight: '700',
    marginBottom: 12,
  },
  bodyText: {
    fontSize: 15,
    lineHeight: 22,
  },
  warningText: {
    fontSize: 15,
    fontWeight: '700',
  },
  spacingTop8: {
    marginTop: 8,
  },
  row: {
    minHeight: 28,
    flexDirection: 'row',
    justifyContent: 'space-between',
    alignItems: 'center',
    gap: 12,
    paddingVertical: 2,
  },
  rowLabel: {
    flex: 1,
    fontSize: 15,
  },
  rowValue: {
    flexShrink: 1,
    textAlign: 'right',
    fontSize: 15,
    fontVariant: ['tabular-nums'],
  },
  footer: {
    borderTopWidth: 1,
    paddingHorizontal: 16,
    paddingTop: 12,
    paddingBottom: 12,
  },
  primaryButton: {
    minHeight: 56,
    borderRadius: 16,
    alignItems: 'center',
    justifyContent: 'center',
  },
  primaryButtonText: {
    fontSize: 17,
    fontWeight: '700',
  },
});
