import { GraphQLClient } from "graphql-request";
import { getSdk } from "~/graphql";

const HASURA_GRAPHQL_URL = "https://cabin.hasura.app/v1/graphql";

export const getHasuraClient = (token: string | null = null) => {
    const headers =
        token !== null
            ? {
                  authorization: `Bearer ${token}`,
              }
            : undefined;
    const client = new GraphQLClient(HASURA_GRAPHQL_URL, {
        headers,
    });
    return getSdk(client);
};

export type HasuraClient = ReturnType<typeof getHasuraClient>;
