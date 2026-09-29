# Motion Filter Selection V2

A V2 do contrato B3b mantém a metodologia offline da V1, mas adiciona vínculo explícito entre política e perfil B2 por fingerprint.

O fingerprint é implementado em `calibration::provenance` para ser compartilhado por seleção offline B3b e runtime B4a sem dependência da seleção sobre o núcleo vivo.

## Diferenças em relação à V1

- `selectionReportVersion = 2` para novos relatórios;
- a política emitida é `TiltEstimatorPolicyV2`;
- `inputs.calibrationProfileFingerprint` registra o perfil realmente usado;
- `recommendation.policy.calibrationProfileFingerprint` repete o mesmo vínculo;
- `TiltEstimatorPolicyV1` permanece representável como histórico, mas é inadequada para ativação viva.

Formato conceitual da política:

```json
{
  "version": 2,
  "candidate": "gravity_no_additional_anchor_filter",
  "parameters": {},
  "yawAvailable": false,
  "source": "b3b_offline_selection",
  "calibrationProfileFingerprint": {
    "version": 1,
    "algorithm": "sha256",
    "digest": "<64 hex lowercase>"
  }
}
```

## Compatibilidade

Relatórios V1 continuam sendo evidência histórica da seleção offline já feita. Eles não possuem vínculo suficiente para ativar o núcleo vivo B4a e não devem ser migrados silenciosamente anexando um perfil arbitrário.

Não houve nova execução física B3b nesta fatia. O teste físico que depende dos artefatos B1/B2 locais continua ignorado quando esses arquivos não existem no checkout.
