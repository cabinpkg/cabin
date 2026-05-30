import type { CodegenConfig } from "@graphql-codegen/cli";

const HASURA_ENDPOINT = "https://cabin.hasura.app/v1/graphql";

const config: CodegenConfig = {
    schema: HASURA_ENDPOINT,
    documents: ["./graphql/**/*.gql"],
    overwrite: true,
    generates: {
        "./graphql/index.ts": {
            plugins: [
                "typescript",
                "typescript-operations",
                "typescript-graphql-request",
            ],
            config: {
                skipTypename: false,
                useTypeImports: true,
                withHooks: false,
                withHOC: false,
                withComponent: false,
            },
        },
        "./graphql.schema.json": {
            plugins: ["introspection"],
        },
    },
};

export default config;
