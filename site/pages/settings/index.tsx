import { useEffect } from "react";
import { useRouter } from "next/router";

import Meta from "~/components/Meta";

export default function Settings(): JSX.Element {
    const router = useRouter();

    // Forward to profile page which is default.
    useEffect(() => {
        router.push("/settings/profile", undefined, { shallow: true });
    }, [router]);

    return (<Meta title="Settings" />);
}
