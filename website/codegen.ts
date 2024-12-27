import type { CodegenConfig } from "@graphql-codegen/cli";

const config: CodegenConfig = {
    schema: "https://cabin.hasura.app/v1/graphql",
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
                withHooks: true,
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
