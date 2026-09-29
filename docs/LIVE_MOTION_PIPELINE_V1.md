# Live Motion Pipeline V1

B4a introduziu um núcleo Rust puro, determinístico e fail-closed para transformar `AcceptedSampleEvent` já aceitos pelo receiver em snapshots observáveis de tilt vivo. B4b liga esse núcleo ao runtime desktop sem mover socket, I/O ou Tauri para o processador puro.

## Fluxo

```text
AcceptedSampleEvent
-> PreparedCalibrationV1
-> controle temporal e sessão
-> estimador B3b V2
-> validade
-> LiveTiltSnapshotV1
```

No desktop B4b, o fluxo operacional é:

```text
datagrama UDP válido
-> receiver aceita a amostra
-> AcceptedSampleSink
-> fila bounded não bloqueante
-> task LiveMotionPipeline
-> get_live_tilt_snapshot
-> seção diagnóstica "Tilt processado"
```

O sink usa `try_send`; se a fila estiver cheia ou fechada, o receiver não bloqueia e o descarte é contabilizado nas métricas de integração. A capacidade centralizada é `LIVE_MOTION_INGRESS_CAPACITY = 120` eventos.

Saída utilizável só existe em `valid`. Em todos os demais estados, `targetTilt` é neutro `(0, 0)`, embora `lastEstimate` possa permanecer para diagnóstico.

## Proveniência

O pipeline exige:

- perfil B2 válido com `yawCalibrated=false`;
- política `TiltEstimatorPolicyV2` válida;
- `calibrationProfileFingerprint` da política igual ao fingerprint do perfil preparado.

No startup desktop, a ativação é explícita pelas variáveis:

- `ANCHOR_CALIBRATION_PROFILE_PATH`;
- `ANCHOR_TILT_POLICY_PATH`.

As duas precisam estar presentes. Não há busca automática por “mais recente”, geração de política V2 a partir de V1, nem anexação de fingerprint artificial. Perfil ou política inválidos deixam o pipeline `unavailable`, mantêm `targetTilt` neutro e preservam o receiver UDP ativo.

Mismatch neutraliza o pipeline como `unavailable` com motivo `calibration_policy_provenance_mismatch`. Não há fallback e política V1 não ativa o núcleo vivo.

O contrato `CalibrationProfileFingerprintV1` vive na camada `calibration::provenance`, compartilhada por B3b e B4a. A API pública calcula o fingerprint sempre validando/canonicalizando o perfil antes do digest; o processador ativo só é criado por `LiveMotionPipeline`, impedindo combinar publicamente perfil e política incompatíveis.

O fingerprint é SHA-256 usado apenas como identificador de consistência de conteúdo, não como autenticação, assinatura ou defesa contra adversários.

## Política temporal inicial

- `MAX_CONTIGUOUS_GAP = 100 ms`;
- `WARM_UP_SAMPLE_COUNT = 2`;
- `STALE_AFTER = 250 ms`;
- `DISCONNECTED_AFTER = 1 s`.

Os limites de 100 ms e duas amostras são políticas iniciais de engenharia, não valores clínicos. `STALE_AFTER` e `DISCONNECTED_AFTER` reutilizam os limites do receiver para evitar definições divergentes.

`dt` vem somente de `sessionElapsedUs`; interarrival vem somente de `received_at`. Lacunas de sequência contam amostras ausentes, mas não fabricam amostras nem forçam reset quando os tempos são contínuos.

## Estados

Estados serializados: `unavailable`, `awaiting_sample`, `warming_up`, `valid`, `invalid`, `stale`, `disconnected`.

Motivos de neutralização são enumerados, incluindo configuração ausente, perfil/política inválidos, mismatch de proveniência, warm-up, mudança de sessão, gap temporal, `dt` inválido, sequência não monotônica, erro de calibração, erro do estimador, stale e disconnected.

Qualquer erro invalidante neutraliza imediatamente, invalida a continuidade temporal anterior, reconstrói o estimador e obriga recuperação por novo warm-up: a próxima amostra utilizável vira a primeira amostra neutra, e só a segunda amostra contínua pode voltar a `valid`.

`lastSampleAgeMs` é opcional: `null`/ausente semanticamente significa nenhuma amostra recebida; `0` significa amostra existente com idade inferior a 1 ms ou snapshot com `now` anterior saturado; valores positivos são idade monotônica local.

`LiveMotionPipeline` é `Send`. A camada runtime B4b adiciona locks, fila e task dedicada fora de `processor.rs`, com encerramento controlado para testes.

## Métricas

As métricas online cobrem contadores de processamento, resets, gaps, erros e acumuladores online de intervalos de origem, interarrival de recepção e variação `receiveDelta - sourceDelta`. Elas não medem latência ponta a ponta.

## Limites explícitos da B4b

Não há overlay transparente, janela always-on-top/click-through, interpolação visual, nova validação física, alegação de redução de cinetose, teste em veículo ou medição formal de taxa, jitter e latência ponta a ponta.
