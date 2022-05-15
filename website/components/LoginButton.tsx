import { LockIcon } from "@chakra-ui/icons";
import { supabaseClient } from "@supabase/supabase-auth-helpers/nextjs";
import { Button } from "@chakra-ui/react";

export default function LoginButton(): JSX.Element {
    return (
        <Button
            leftIcon={<LockIcon />}
            variant="outline"
            onClick={(): void => {
                supabaseClient.auth.signIn(
                    { provider: "github" },
                );
            }}
        >
            Log in with GitHub
        </Button>
    );
}
