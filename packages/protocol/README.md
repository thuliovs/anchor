# @anchor/protocol

Contrato versionado e independente de linguagem para amostras de movimento do Anchor.

## Sistema de coordenadas

Todos os vetores usam o referencial assumido pela convencao de montagem atual:

- X positivo = direita
- Y positivo = frente
- Z positivo = cima

A convencao fisica obrigatoria nesta fase e:

- telefone deitado;
- tela para cima;
- modo retrato;
- borda superior apontando para a frente do veiculo.

Esse referencial ainda nao foi calibrado nem verificado empiricamente para todos os cenarios. Os datasets da fase B1 existem justamente para confirmar os sinais reais dos eixos antes da futura operacao de zero/calibracao.

## Unidades

- `linearAccelerationMps2`: metros por segundo ao quadrado, sem gravidade
- `gravityMps2`: metros por segundo ao quadrado
- `angularVelocityRadS`: radianos por segundo

## Versionamento

O JSON Schema em `schema/motion-sample.v1.schema.json` e a constante `protocolVersion: 1` sao a especificacao normativa do protocolo v1. Mudancas incompativeis devem criar uma nova versao, por exemplo `protocolVersion: 2`.

## Relogio monotonico

`sessionElapsedUs` mede o tempo decorrido desde o inicio da sessao, em microssegundos. Nao e timestamp Unix nem hora do sistema.

## sessionId

`sessionId` e apenas um identificador opaco de sessao. Nao representa autenticacao nem autorizacao.

## Limite de datagrama

`MAX_DATAGRAM_BYTES` define o limite futuro de 1024 bytes para um datagrama do protocolo. Este pacote nao implementa transporte.

## Testes

- TypeScript: `pnpm --filter @anchor/protocol typecheck`
- Schema/fixtures TypeScript: `pnpm --filter @anchor/protocol test`
- Rust: `cargo test protocol --manifest-path apps/desktop/src-tauri/Cargo.toml`
