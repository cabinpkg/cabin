import { ChevronDownIcon } from "@chakra-ui/icons";
import {
    Avatar,
    Button,
    Center,
    Menu,
    MenuButton,
    MenuDivider,
    MenuItem,
    MenuList,
    Text,
} from "@chakra-ui/react";
import { useCallback } from "react";

// import LoginButton from "~/components/LoginButton";
import { Link } from "~/components/Link";

// from: supabase (gotrue-js)
// TODO: Customize for Poac (there should be unnecessary fields)
interface User {
    id: string;
    app_metadata: {
        provider?: string;
        // biome-ignore lint/suspicious: intended
        [key: string]: any;
    };
    user_metadata: {
        // biome-ignore lint/suspicious: intended
        [key: string]: any;
    };
    aud: string;
    confirmation_sent_at?: string;
    recovery_sent_at?: string;
    email_change_sent_at?: string;
    new_email?: string;
    invited_at?: string;
    action_link?: string;
    email?: string;
    phone?: string;
    created_at: string;
    confirmed_at?: string;
    email_confirmed_at?: string;
    phone_confirmed_at?: string;
    last_sign_in_at?: string;
    role?: string;
    updated_at?: string;
}

interface UserProps {
    user: User;
}

function User(props: UserProps): JSX.Element {
    const signOut = useCallback(async (): Promise<void> => {
        alert("Sign out is not implemented.");
    }, []);

    return (
        <Menu>
            <MenuButton as={Button}>
                <Center>
                    <Avatar
                        size="xs"
                        name={props.user.user_metadata.name}
                        src={props.user.user_metadata.avatar_url}
                        marginRight={1}
                    />
                    <Text>{props.user.user_metadata.name}</Text>
                    <ChevronDownIcon />
                </Center>
            </MenuButton>
            <MenuList>
                <MenuItem as={Link} href="/dashboard">
                    Dashboard
                </MenuItem>
                <MenuItem as={Link} href="/settings">
                    Settings
                </MenuItem>
                <MenuDivider />
                <MenuItem onClick={signOut}>Sign Out</MenuItem>
            </MenuList>
        </Menu>
    );
}

export default function UserMenu(): JSX.Element {
    // const { user } = useUser();

    // return (
    //     user ? <User user={user} /> : <LoginButton />
    // );
    return <></>;
}
