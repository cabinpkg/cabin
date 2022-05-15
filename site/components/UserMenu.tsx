import { useUser } from "@supabase/supabase-auth-helpers/react";
import { Avatar, Center, Text, Button, Menu, MenuButton, MenuList, MenuItem, MenuDivider } from "@chakra-ui/react";
import { ChevronDownIcon } from "@chakra-ui/icons";
import { User } from "@supabase/supabase-js";
import { supabaseClient } from "@supabase/supabase-auth-helpers/nextjs";

import LoginButton from "./LoginButton";

interface UserProps {
    user: User;
}

function User(props: UserProps): JSX.Element {
    return (
        <Menu>
            <MenuButton as={Button}>
                <Center>
                    <Avatar
                        size="xs"
                        name={props.user.user_metadata["name"]}
                        src={props.user.user_metadata["avatar_url"]}
                        marginRight={1}
                    />
                    <Text>{props.user.user_metadata["name"]}</Text>
                    <ChevronDownIcon />
                </Center>
            </MenuButton>
            <MenuList>
                <MenuItem>Dashboard</MenuItem>
                <MenuItem>Settings</MenuItem>
                <MenuDivider />
                <MenuItem onClick={async (): Promise<void> => {
                    await supabaseClient.auth.signOut();
                }}
                >Sign Out</MenuItem>
            </MenuList>
        </Menu>
    );
}

export default function UserMenu(): JSX.Element {
    const { user } = useUser();

    return (
        user ? <User user={user} /> : <LoginButton />
    );
}
