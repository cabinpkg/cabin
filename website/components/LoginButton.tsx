import { LockIcon } from "@chakra-ui/icons";
import { Button } from "@chakra-ui/react";
import { useCallback } from "react";

export default function LoginButton(): JSX.Element {
    const signIn = useCallback(async (): Promise<void> => {
        alert("Sign in is not implemented.");
    }, []);

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
