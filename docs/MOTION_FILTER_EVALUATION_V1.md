# Motion Filter Evaluation V1

Harness offline, deterministico e reproduzivel da fatia B3a para comparar estimadores de tilt observavel, roll e pitch, sem escolher vencedor nem parametros de producao.

## Objetivo

A B3a fornece uma bancada Rust pura para executar os mesmos sinais calibrados em tres candidatos:

- gravidade calibrada sem filtro adicional do Anchor;
- passa-baixa vetorial de primeira ordem;
- complementar entre giroscopio e direcao da gravidade.

O resultado e um relatorio humano compacto e um JSON versionado. A decisao de vencedor, parametros finais e aplicacao ao receptor ao vivo pertencem a B3b ou fatias posteriores.

## Arquitetura

O modulo `apps/desktop/src-tauri/src/motion_filtering/` separa responsabilidades:

- `estimator.rs`: APIs puras, convencoes matematicas, validacao de `dt`, baseline, passa-baixa e complementar;
- `synthetic.rs`: fixtures sinteticas deterministicas com ground truth independente;
- `metrics.rs`: metricas com ground truth e proxies fisicos;
- `report.rs`: contrato JSON v1 e relatorio humano;
- `mod.rs`: replay, aplicacao da calibracao B2 e orquestracao.

O parser/validador B1 e reutilizado por `load_validated_dataset_file` e `analyze_dataset_file`. A calibracao B2 e reutilizada pelas APIs `apply_calibration_to_linear`, `apply_calibration_to_angular` e `apply_calibration_to_gravity`. A B3a nao duplica as formulas da B2.

O replay sintetico e fisico usa o mesmo pipeline conceitual:

```text
raw device sample
-> perfil B2 valido
-> calibrated sample
-> estimador
-> metricas
```

Nas fixtures sinteticas cada fixture possui seu proprio perfil B2. A maioria usa perfil identidade valido gerado pela propria B2; `mounting_bias_b2` usa perfil nao identidade com montagem e biases conhecidos.

## Convencoes

Depois da calibracao B2:

- X positivo = direita;
- Y positivo = frente;
- Z positivo = cima;
- em repouso nivelado, `normalize(gravityCalibrated)` e aproximadamente `(0, 0, -1)`.

Usam-se vetores coluna e rotacoes ativas:

```text
v' = q * v * q^-1
```

O vetor de direcao da gravidade e:

```text
d = normalize(gravityCalibrated)
```

Roll e pitch sao derivados sem yaw:

```text
rollRad  = atan2(d.x, sqrt(d.y^2 + d.z^2))
pitchRad = atan2(d.y, -d.z)
```

Assim, `gravity.x > 0` produz roll positivo, `gravity.y > 0` produz pitch positivo e `(0, 0, -1)` produz zero. Esses angulos descrevem o tilt observavel no dominio normal de montagem, nao uma orientacao 3D completa.

As fixtures declaram diretamente `rollTruth(t)`, `pitchTruth(t)` e `yawTruth(t)`. A orientacao sintetica e construida por:

```text
qDeviceToWorld = qYawAboutPositiveZ * qRollAboutPositiveY * qPitchAboutNegativeX
```

Equivalente em matrizes:

```text
RDeviceToWorld = Rz(yaw) * Ry(roll) * Rx(-pitch)
```

Essa composicao garante que roll positivo gere `gravityDevice.x > 0`, pitch positivo gere `gravityDevice.y > 0`, yaw puro nao altere a gravidade e nivel produza `(0, 0, -1)`.

## Estimadores

### Gravidade Sem Filtro Adicional

Por amostra:

```text
dEstimated = normalize(gravityCalibrated)
```

O nome evita dizer “gravidade bruta”: Android `TYPE_GRAVITY` pode conter fusao ou filtragem do fabricante. Aqui significa apenas sem filtro adicional do Anchor.

### Passa-Baixa Vetorial

Parametro: `tauSeconds > 0` finito.

Inicializacao:

```text
gFiltered0 = gravityCalibrated0
```

Atualizacao com `dt` real:

```text
retention = exp(-dt / tau)
injection = 1 - retention
gFiltered = retention * gFilteredPrevious + injection * gravityCalibrated
dEstimated = normalize(gFiltered)
```

A implementacao calcula `injection` com `-expm1(-dt / tau)` e preserva magnitude no estado vetorial.

### Complementar Gravity + Gyro

Parametro: `correctionTauSeconds > 0` finito.

O estado e somente `dEstimated`, uma direcao unitaria da gravidade no frame calibrado. Yaw nao e mantido nem publicado.

O giroscopio esta no frame corporal/calibrado atual. Para propagar um vetor inercial fixo expresso nesse frame usa-se o sinal negativo da rotacao corporal:

```text
rotationVector = -omega * dt
```

A propagacao usa integracao exponencial assumindo velocidade angular constante no intervalo. Para rotacoes pequenas usa identidade; caso contrario aplica Rodrigues, equivalente ao quaternion ativo `qDelta * d * qDelta^-1`.

A correcao pela gravidade e:

```text
beta = 1 - exp(-dt / correctionTau)
```

`beta` tambem usa `-expm1(-dt / correctionTau)`. A correcao e interpolacao vetorial normalizada, nao SLERP, e pode ser limitada perto de direcoes antipodais. O dominio normal de montagem nao deve chegar a esse caso.

## API Temporal

A primeira amostra inicializa pela direcao da gravidade e nao requer `dt`. Toda atualizacao posterior recebe `dtSeconds` explicito calculado de `sessionElapsedUs`:

```text
dtSeconds = (current.sessionElapsedUs - previous.sessionElapsedUs) / 1_000_000
```

`dt == 0`, `dt < 0`, `NaN`, `+inf` e `-inf` sao rejeitados diretamente pelos estimadores. A B3a nao aplica limite maximo arbitrario de `dt`; politicas de gap, stale e disconnect ficam fora desta fatia.

## Fixtures Sinteticas

As fixtures usam ground truth independente como trajetoria `qDeviceToWorld(t)`. A gravidade no mundo e `(0, 0, -g)` e o sinal ideal no dispositivo e:

```text
gravityDevice(t) =
  qDeviceToWorld(t)^-1
  * gWorld
  * qDeviceToWorld(t)
```

O giroscopio vem do delta de orientacao entre amostras:

```text
qDeltaBody = qPrevious^-1 * qCurrent
omegaBody = rotationVector(qDeltaBody) / dt
```

O log usa a solucao de menor angulo, canonicalizando `w >= 0`. A velocidade angular da amostra atual representa o intervalo anterior para atual, compativel com o replay. O giroscopio nao e derivado da gravidade nem da saida dos estimadores.

O ground truth de roll/pitch nao chama `estimate_from_direction`, nenhum `Estimator` e nenhuma API de producao avaliada. Roll e pitch verdadeiros sao os angulos declarados pela propria trajetoria sintetica; a direcao verdadeira da gravidade e gerada por quaternion.

Suite essencial implementada:

- repouso nivelado perfeito;
- repouso com tilt conhecido;
- degrau de roll positivo e negativo;
- degrau de pitch positivo e negativo;
- rampa de roll;
- senoide de roll;
- senoide de pitch;
- trajetoria combinada de roll e pitch;
- yaw puro sem tilt relevante;
- bias constante de giroscopio;
- pulso de contaminacao da gravidade;
- `dt` irregular deterministico;
- fixture de montagem e bias para provar aplicacao B2.

As fixtures simples usam perfil identidade valido gerado pela propria calibracao B2 a partir de dataset estacionario sintetico. A fixture de montagem/bias gera um dataset estacionario raw com inclinacao e biases conhecidos, gera um perfil B2 valido, usa a rotacao `deviceToLeveled` efetivamente presente no perfil e gera uma trajetoria dinamica raw por:

```text
gravityRaw = C^-1 * gravityLeveled
angularRaw = angularBiasDevice + C^-1 * angularLeveled
linearRaw  = linearBiasDevice + C^-1 * linearLeveled
```

O harness aplica B2 durante o replay, antes dos estimadores, e os testes provam que os sinais calibrados recuperam os canonicos.

Cada fixture tambem declara metadata de evento quando aplicavel:

- `Step`: eixo, onset, angulo inicial, alvo e banda de acomodacao;
- `Sine`: eixo, inicio, fim e frequencia;
- `GravityContamination`: inicio, fim, banda de recovery e duracao minima de permanencia.

## Metricas

Com ground truth, a metrica angular primaria e:

```text
angularError = acos(clamp(dot(dEstimated, dTruth), -1, 1))
```

O relatorio agrega RMSE, p95 R-7 e maximo em graus. Roll e pitch usam diferenca angular com wrap. Tambem sao reportadas metricas estacionarias basicas e metricas temporais quando aplicaveis.

### Metricas temporais

Overshoot de degrau usa o eixo declarado pelo evento:

```text
delta = target - initial
direction = sign(delta)
signedExcess = direction * wrap(outputAngle - target)
overshoot = max(0, max(signedExcess after onset))
```

Um baseline que acompanha exatamente o degrau tem `overshootDeg = 0`, inclusive para degraus negativos.

Settling time usa a banda declarada, inicialmente 2 graus, e retorna tempo relativo ao evento:

```text
settlingTimeSeconds = sampleTime - onsetSeconds
```

A amostra precisa estar dentro da banda e permanecer dentro dela ate o fim do segmento de hold. Se nao acomodar, a metrica fica indisponivel.

Sine lag usa projecao senoidal na janela declarada. A media e removida e a fase fundamental e:

```text
sinProjection = sum(x(t) * sin(omega * t))
cosProjection = sum(x(t) * cos(omega * t))
phase = atan2(cosProjection, sinProjection)
phaseLag = wrapPi(phaseTruth - phaseEstimate)
lagSeconds = phaseLag / omega
```

Lag positivo significa que a estimativa esta atrasada em relacao ao truth nessa convencao. Amplitude fundamental degenerada retorna indisponivel.

Recovery time usa o fim da perturbacao como origem, erro angular contra ground truth, banda de 0,5 grau na fixture atual e duracao minima de permanencia:

```text
recoveryTimeSeconds = sampleTime - contaminationEndSeconds
```

Se nao houver segmento suficiente ou nao recuperar, a metrica fica indisponivel. Nenhuma metrica temporal reutiliza um helper generico de maximo com unidade errada.

Quando uma metrica nao se aplica, o JSON usa `status: unavailable` com motivo, nunca zero enganoso.

Para datasets fisicos sem ground truth, o JSON define `groundTruthAvailable: false` e usa apenas proxies comportamentais:

- dispersao angular em relacao a direcao media;
- RMS da taxa de variacao angular;
- pico de tilt relativo ao estado inicial;
- distancia final-inicial;
- divergencia media entre candidatos.

Esses proxies nao sao precisao, acuracia nem RMSE contra verdade.

## JSON V1

O relatorio usa `evaluationReportVersion = 1` e campos `camelCase`, incluindo:

- `input`;
- `groundTruthAvailable`;
- `yawCalibrated`;
- `sampleCount`;
- `observedDurationUs`;
- `dtStatistics`;
- `calibration`;
- `configurations[]`;
- `warnings[]`;
- `limitations[]`.

Cada configuracao tem:

- `candidate`;
- `parameters`;
- `summary`;
- `fixtureResults[]` em suites sinteticas.

`fixtureResults[]` preserva metricas de cada fixture separadamente, incluindo degraus, senoides, pulso de gravidade, bias de gyro, `dt` irregular e `mounting_bias_b2`. Metricas nao aplicaveis usam `status: unavailable`.

O resumo global sintetico e calculado a partir dos erros por amostra acumulados:

```text
globalRMSE = sqrt(sum(error^2) / totalSampleCount)
globalP95 = percentileR7(allSampleErrors)
globalMax = max(allSampleErrors)
```

Metricas de evento, como overshoot, settling, lag e recovery, permanecem por fixture e nao sao misturadas num agregado sem semantica clara.

O JSON nao inclui timestamp atual, benchmark, caminho absoluto, `NaN` ou infinito. A ordem e deterministica e nao ha recomendacao de vencedor.

## CLI

Suite sintetica:

```bash
pnpm motion:evaluate -- \
  --synthetic \
  --low-pass-tau-ms 50,100,200,400 \
  --complementary-tau-ms 100,250,500,1000
```

Dataset fisico:

```bash
pnpm motion:evaluate -- \
  --dataset artifacts/motion-datasets/20260902T221420Z-stationary.ndjson \
  --profile artifacts/motion-calibrations/20260902t221420z-stationary-calibration-v1.json \
  --low-pass-tau-ms 50,100,200,400 \
  --complementary-tau-ms 100,250,500,1000
```

Adicione `--json` para JSON limpo em stdout. Erros vao para stderr e retornam codigo diferente de zero.

## Capturas Fisicas

A avaliacao das nove capturas B1 selecionadas usa o perfil B2 estacionario e assume:

- mesmo telefone;
- mesma convencao de montagem;
- posicao fisica do aparelho preservada materialmente entre capturas.

Sem essa premissa, os proxies fisicos perdem comparabilidade. Mesmo com a premissa, eles nao representam precisao angular absoluta.

## Limitacoes

- `TYPE_GRAVITY` e `TYPE_LINEAR_ACCELERATION` sao sensores virtuais Android e podem conter processamento do fabricante;
- “sem filtro adicional” nao significa sinal fisico bruto;
- o payload nao contem timestamps individuais dos tres sensores;
- nao e possivel alegar atraso fisico rigoroso entre gravity e gyro;
- yaw nao e observavel nesta arquitetura;
- nao ha magnetometro;
- nao ha correcao adaptativa por aceleracao linear;
- nao ha receptor ao vivo, UI, overlay, politica de gaps, stale ou disconnect nesta fatia;
- nao ha escolha automatica de vencedor.

## Relacao B3a/B3b

B3a cria a bancada, os candidatos minimos, as fixtures, metricas e contratos de saida. B3b deve usar essa evidencia para decidir parametros, politica de producao e possivel integracao posterior, sem reinterpretar proxies fisicos como verdade absoluta.
