import { GraphQLClient } from "graphql-request";
import { getSdk } from "../../graphql";
import { HASURA_ENDPOINT } from "./constants";

export function getHasuraClient(token: string | null = null) {
    const headers =
        token !== null
            ? {
                  authorization: `Bearer ${token}`,
              }
            : undefined;

    return getSdk(new GraphQLClient(HASURA_ENDPOINT, { headers }));
}

export type HasuraClient = ReturnType<typeof getHasuraClient>;
