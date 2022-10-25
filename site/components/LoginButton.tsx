import { LockIcon } from "@chakra-ui/icons";
import { supabaseClient } from "@supabase/supabase-auth-helpers/nextjs";
import { Button } from "@chakra-ui/react";
import { useCallback } from "react";
import { useRouter } from "next/router";

import { BASE_URL } from "~/utils/constants";

export default function LoginButton(): JSX.Element {
    const router = useRouter();
    
    const signIn = useCallback(async (): Promise<void> => {
        await supabaseClient.auth.signIn(
            { provider: "github" },
            {
                redirectTo: BASE_URL + router.asPath,
            }
        );
    }, [router.asPath]);

    return (
        <Button
            leftIcon={<LockIcon />}
            variant="outline"
            onClick={signIn}
            disabled
        >
            Log in with GitHub
        </Button>
    );
}
