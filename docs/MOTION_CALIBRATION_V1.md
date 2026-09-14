# Motion Calibration V1

Calibracao/zero offline, deterministica e versionada para o Project Anchor a partir de um dataset `stationary`.

Esta fatia B2 nao aplica o perfil ao fluxo ao vivo, nao altera o transporte UDP e nao constitui validacao dinamica em veiculo nem validacao de eficacia contra cinetose.

## Objetivo

A calibracao v1 estima um `leveled mounting frame` a partir de uma captura estatica para:

1. estimar o vetor medio de gravidade no frame do dispositivo;
2. alinhar essa gravidade com `(0, 0, -|g|)` preservando a magnitude medida;
3. estimar bias estacionario da aceleracao linear;
4. estimar bias estacionario da velocidade angular;
5. produzir um perfil JSON reutilizavel;
6. expor uma API Rust pura e testavel para aplicar o perfil;
7. reportar residuos apos a propria transformacao.

## Limitacoes importantes

- a gravidade corrige apenas roll e pitch;
- `yawCalibrated` e sempre `false`;
- X e Y continuam dependentes da convencao de montagem: telefone deitado, tela para cima, retrato, borda superior apontando para a frente;
- nao ha magnetometro;
- nao ha inferencia de direcao por movimento;
- o resultado nao deve ser chamado de referencial completo do veiculo;
- o zero nao distingue inclinacao do suporte, do veiculo e do piso/estrada no instante da captura.

## Convencao de quaternion

O perfil serializa `deviceToLeveledQuaternion` com campos `w`, `x`, `y`, `z`.

Regras da v1:

- quaternion unitario;
- rotacao ativa;
- aplicacao documentada como `v' = q · v · q^-1`;
- serializacao canonica com preferencia por `w >= 0`;
- quando `w` e aproximadamente zero, a canonicidade usa o primeiro componente significativo positivo entre `x`, `y` e `z`.

## Equacoes

Se `g_mean` e a media vetorial das amostras de `gravityMps2`, a rotacao e calculada para satisfazer:

```text
normalize(g_mean) -> (0, 0, -1)
```

Sem forcar `|g| = 9.80665`.

Aplicacao do perfil:

```text
linearCalibrated =
  R(deviceToLeveled) * (linearRaw - deviceFrameLinearAccelerationBiasMps2)

angularCalibrated =
  R(deviceToLeveled) * (angularRaw - deviceFrameAngularVelocityBiasRadS)

gravityCalibrated =
  R(deviceToLeveled) * gravityRaw
```

A gravidade nao e subtraida de `gravityMps2`.

## Quality Gate v1

Os limites abaixo ficam centralizados em constantes nomeadas no modulo Rust. Eles sao criterios diagnosticos iniciais da calibracao v1; nao sao limites clinicos nem garantias de eficacia.

- `scenario == stationary`;
- dataset completo;
- exatamente uma sessao;
- pelo menos 180 amostras;
- pelo menos 3 segundos observados;
- `recorderDroppedSamples == 0`;
- no maximo 1% de amostras ausentes por lacunas de sequencia;
- taxa media de origem entre 45 e 75 Hz;
- magnitude media da gravidade entre 9.0 e 10.5 m/s^2;
- desvio-padrao da magnitude da gravidade <= 0.10 m/s^2;
- RMS da magnitude da aceleracao linear <= 0.35 m/s^2;
- RMS da magnitude da velocidade angular <= 0.10 rad/s;
- correcao de inclinacao <= 30 graus.

Quando ha multiplas violacoes, o relatorio retorna todas elas.

## Perfil JSON

Contrato Rust estrito com `deny_unknown_fields`.

Campos principais:

- `calibrationProfileVersion = 1`;
- `method = stationary_level_and_bias_v1`;
- `createdAtUtc` com o instante real de criacao do perfil;
- `sourceStartedAtUtc` com o timestamp original `startedAtUtc` do metadata do dataset;
- `sourceDataset` com nome seguro relativo, nunca caminho absoluto;
- `sourceDatasetFormatVersion`;
- `sourceProtocolVersion`;
- `sourceSessionId`;
- `sourceSampleCount`;
- `sourceObservedDurationUs`;
- `mountingConvention = leveled_mounting_frame_screen_up_portrait_top_toward_vehicle_front_yaw_unconstrained_v1`;
- `yawCalibrated = false`;
- `deviceFrameMeanGravityMps2`;
- `deviceFrameLinearAccelerationBiasMps2`;
- `deviceFrameAngularVelocityBiasRadS`;
- `gravityMagnitudeMeanMps2`;
- `deviceToLeveledQuaternion`;
- `tiltCorrectionDegrees`;
- `quality` com diagnosticos, violacoes e residuos.

A leitura valida:

- versao;
- metodo;
- `yawCalibrated == false`;
- `sourceDataset` seguro;
- versoes de origem suportadas;
- consistencia entre campos de origem, diagnosticos, quaternion, gravidade media e residuos;
- `quality.passed == true` e `quality.violations` vazio;
- numeros finitos;
- quaternion unitario dentro da tolerancia documentada.

A matematica da calibracao permanece deterministica para o mesmo dataset e mesmo instante de criacao injetado. A CLI usa o relogio atual, portanto `createdAtUtc` varia entre execucoes reais.

## CLI

Comandos:

```bash
pnpm motion:calibrate -- <stationary.ndjson>
pnpm motion:calibrate -- <stationary.ndjson> --output <profile.json>
pnpm motion:calibrate -- <stationary.ndjson> --output <profile.json> --json
```

Comportamento:

- caminho padrao em `artifacts/motion-calibrations/`;
- nome derivado com seguranca do dataset;
- sem sobrescrever perfil existente;
- escrita atomica sem deixar arquivo parcial em falha;
- saida humana com biases, quaternion, inclinacao, `yawCalibrated=false`, qualidade e residuos;
- `--json` produz JSON apropriado para automacao;
- falhas retornam codigo diferente de zero.

## Residuos reportados

Depois de estimar o perfil, o proprio dataset e recalibrado offline e o relatorio inclui:

- media da gravidade calibrada por eixo;
- magnitude media da gravidade calibrada;
- media e RMS da aceleracao linear calibrada;
- media e RMS da velocidade angular calibrada;
- erro angular residual da gravidade em relacao a `(0, 0, -1)`.

Esses residuos servem apenas para verificar a calibracao estatica. Nao ha filtro, fusao, orientacao integrada ou compensacao dinamica.

## Evidencia fisica offline B2

O pipeline offline B2 foi validado com o dataset estacionario real ignorado pelo Git:

```text
artifacts/motion-datasets/20260902T221420Z-stationary.ndjson
```

Resumo da execucao:

- `quality.passed: true`;
- amostras: `902`;
- duracao observada: `14.986.690 us`;
- taxa de origem: `60,0254 Hz`;
- correcao de inclinacao: `5,6488°`;
- gravidade media: `9,8598 m/s^2`;
- RMS linear calibrado: `0,0279031 m/s^2`;
- RMS angular calibrado: `0,00648794 rad/s`;
- erro angular residual RMS da gravidade: `0,260718°`;
- `yawCalibrated: false`.

Essa evidencia valida a geracao offline do perfil, suas transformacoes estaticas e os residuos reportados pela B2. Ela nao valida aplicacao ao fluxo ao vivo, comportamento dinamico em veiculo, filtros, fusao, overlay ou eficacia contra cinetose.

## API Rust

O modulo `apps/desktop/src-tauri/src/calibration/` concentra:

- estimativa da calibracao;
- quality gate;
- tipos do perfil;
- leitura e escrita do perfil;
- aplicacao pura do perfil a vetores;
- relatorio de residuos.

APIs principais:

- `calibrate_dataset_file`;
- `calibrate_dataset_str`;
- `load_calibration_profile_file`;
- `load_calibration_profile_str`;
- `write_calibration_profile_file`;
- `apply_calibration_to_linear`;
- `apply_calibration_to_angular`;
- `apply_calibration_to_gravity`.

## Interpretacao pratica

- bias linear perto de zero: suporte estavel e dataset realmente parado;
- bias angular perto de zero: ausencia de rotacao residual relevante;
- media calibrada da gravidade proxima de `(0, 0, -|g|)`: nivelamento coerente;
- erro angular residual baixo: dataset estacionario consistente;
- violacoes do quality gate: recalibrar com captura mais longa e mais estavel.

## Fora de escopo nesta versao

- aplicacao ao fluxo ao vivo;
- persistencia/selecionador de perfil na UI;
- magnetometro;
- estimacao de yaw;
- fusao de sensores;
- filtros;
- integracao de orientacao;
- compensacao dinamica de drift;
- alegacoes clinicas.
