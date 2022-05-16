import { LockIcon } from "@chakra-ui/icons";
import { supabaseClient } from "@supabase/supabase-auth-helpers/nextjs";
import { Button } from "@chakra-ui/react";
import { useCallback } from "react";

export default function LoginButton(): JSX.Element {
    const signIn = useCallback((): void => {
        supabaseClient.auth.signIn(
            { provider: "github" },
        );
    }, []);

    return (
        <Button
            leftIcon={<LockIcon />}
            variant="outline"
            onClick={signIn}
        >
            Log in with GitHub
        </Button>
    );
}
