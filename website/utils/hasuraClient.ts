import { GraphQLClient } from "graphql-request";
import { getSdk } from "../graphql/graphql";
import { HASURA_GRAPHQL_URL } from "./constants";

export const createHasuraClient = (token: string | null = null) => {
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

export type HasuraClient = ReturnType<typeof createHasuraClient>;
