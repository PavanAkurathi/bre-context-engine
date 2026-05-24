# Enterprise Pre-AI Corpus Strategy

Use `benchmarks/enterprise-pre-ai-repos.json` to expand BRE beyond the current
Rust/TypeScript game-framework stress suite.

## Snapshot Dates

- `2020-12-31`: clean pre-AI baseline.
- `2023-01-01`: transition comparison point.

## Target Languages

Enterprise code is not represented well by public GitHub popularity alone. BRE
should cover these families explicitly:

- Java and JVM systems first.
- C#/.NET.
- C and C++ systems.
- Python services, automation, and data/workflow code.
- JavaScript/TypeScript frontend and Node surfaces.
- Go for cloud-native infrastructure such as Kubernetes, Helm, Terraform, and
  Grafana-style service stacks.

## Initial 2020 Audit Sample

Command:

```sh
python3 benchmarks/audit_repo_languages.py \
  --suite benchmarks/enterprise-pre-ai-repos.json \
  --date 2020-12-31 \
  --limit 6
```

Result:

| Repo | 2020 Snapshot | Dominant Languages By File Count |
| --- | --- | --- |
| `spring-projects/spring-boot` | `8cd07dbc60` | Java 79%, Other 12%, XML 3%, JSON 1%, YAML 1% |
| `spring-projects/spring-framework` | `5c1176786c` | Java 85%, XML 7%, Other 5%, Kotlin 1%, SQL 1% |
| `apache/kafka` | `42272f0a37` | Java 71%, Scala 14%, Other 5%, JSON 4%, Python 3% |
| `apache/cassandra` | `200615ded5` | Java 74%, Other 21%, reStructuredText 3%, Python 1%, YAML 1% |
| `elastic/elasticsearch` | `e26c9bbd529` | Java 71%, Other 21%, YAML 5%, JSON 3%, XML 0% |
| `keycloak/keycloak` | `03e7ec760c` | TypeScript 61%, JSON 14%, Other 10%, CSS 5%, JavaScript 5% |

## Where Kubernetes, Kafka, And Frameworks Fit

Kubernetes, Kafka, Spring, ASP.NET, Django, Airflow, Helm, Terraform, and similar
systems are **context boundary layers**, not just code libraries.

They matter because the correct edit is often split across:

- application source;
- dependency injection or route/controller wiring;
- build manifests such as `pom.xml`, `build.gradle`, `.csproj`, `go.mod`, or
  `pyproject.toml`;
- runtime config such as `application.yml`, `appsettings.json`, `settings.py`,
  or Kafka `.properties`;
- deployment config such as Kubernetes YAML, Helm `Chart.yaml`, Helm
  `values.yaml`, Terraform `.tf`, Docker, and CI workflows;
- schema or protocol files such as `.proto`, SQL migrations, OpenAPI specs, or
  generated API types.

BRE should treat these as high-risk context surfaces. Even before full Java,
C#, Python, C++, and Go AST support lands, pulling these boundary files into
Watching prevents an agent from editing implementation code while blind to the
runtime contract around it.

## Immediate Engine Implication

The current engine now recognizes more enterprise framework and platform files
as critical or active-nearest manifest context:

- Java/JVM: `pom.xml`, `build.gradle`, `settings.gradle`, `gradle.properties`,
  `application.yml`, `application.properties`, `bootstrap.yml`, and
  `bootstrap.properties`.
- .NET: `.sln`, `.csproj`, `.fsproj`, `global.json`,
  `Directory.Build.props`, `Directory.Build.targets`, and `appsettings.json`.
- Python: `pyproject.toml`, `requirements.txt`, `Pipfile`, `poetry.lock`,
  `setup.py`, and `setup.cfg`.
- C/C++ and build graphs: `CMakeLists.txt`, `BUILD`, `BUILD.bazel`,
  `MODULE.bazel`, `WORKSPACE`, and `WORKSPACE.bazel`.
- Kubernetes/Helm: `k8s/**.yaml`, `kubernetes/**.yaml`, `deploy/**.yaml`,
  `deployments/**.yaml`, `helm/**.yaml`, `charts/**.yaml`, `Chart.yaml`, and
  `values*.yaml`.
- Kafka: `server.properties`, `producer.properties`, `consumer.properties`,
  `connect-*.properties`, `schema-registry.properties`, and
  `kafka/**/*.{properties,yaml,json}`.

BRE now also has a first Java parser/resolver pass:

- Parses Java import declarations with `tree-sitter-java`.
- Resolves Maven/Gradle-style project imports through `src/main/java`,
  `src/test/java`, and common generated/integration source roots.
- Resolves static imports back to the owning Java class when possible.
- Includes Java files in symbol search and large-file skeletonization.

This is not yet a full JVM language server. Maven/Gradle dependency graphs,
generated sources, annotation processors, Kotlin/Groovy source sets, and
cross-repo binary dependencies still need explicit modeling.

## Next Parser Order

1. True TypeScript/TSX parser support with `tree-sitter-typescript`.
2. Deeper Java support: Maven/Gradle module graphs, generated sources, and
   package-private same-package dependencies.
3. Python parser and import/package resolution.
4. C# parser and MSBuild project resolution.
5. C/C++ parser plus compile database / CMake / Bazel awareness.
6. Go parser for Kubernetes/Terraform/Grafana-style cloud-native repos.
