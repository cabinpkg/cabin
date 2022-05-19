import { useUser } from "@supabase/supabase-auth-helpers/react";
import { useRouter } from "next/router";
import type { ChangeEvent } from "react";
import { useCallback, useEffect, useState } from "react";
import {
    useDisclosure,
    useClipboard,
    Tabs,
    TabList,
    Tab,
    TabPanels,
    TabPanel,
    Center,
    Avatar,
    VStack,
    Heading,
    HStack,
    Text,
    Spacer,
    Button,
    Modal,
    ModalOverlay,
    ModalContent,
    ModalHeader,
    ModalCloseButton,
    ModalBody,
    ModalFooter, Input, Spinner, Box, Tooltip, Code, Divider,
} from "@chakra-ui/react";
import { AddIcon } from "@chakra-ui/icons";
import type { GetServerSideProps } from "next";
import humanizeString from "humanize-string";
import type { User } from "@supabase/supabase-js";
import randomstring from "randomstring";
import { supabaseClient } from "@supabase/supabase-auth-helpers/nextjs";
import { format } from "timeago.js";

import Meta from "~/components/Meta";
import NeedAuth from "~/components/NeedAuth";
import type { uuid } from "~/utils/types";

const tabs = ["profile", "tokens"] as const;
type TabsType = typeof tabs[number];

interface TokenType {
    id: uuid;
    created_at: string;
    last_used_at?: string;
    name: string;
    token?: string;
}

interface TokenProps {
    token: TokenType;
    onRevoke: () => void;
}

function Token(props: TokenProps): JSX.Element {
    const { hasCopied, onCopy } = useClipboard(props.token.token ?? "");

    return (
        <Box borderWidth="1px" borderRadius="md" padding={5} width="100%">
            <HStack>
                <VStack align="left">
                    <Text as="b" fontSize="lg">{props.token.name}</Text>
                    <Text>
                        {props.token.last_used_at ?
                            `Last used ${format(new Date(props.token.last_used_at))}` :
                            "Never used"}
                    </Text>
                    <Text>
                        {`Created ${format(new Date(props.token.created_at))}`}
                    </Text>
                </VStack>
                <Spacer />
                <Button onClick={props.onRevoke}>Revoke</Button>
            </HStack>
            {props.token.token &&
                <VStack marginTop={5}>
                    <Divider />
                    <Text as="b" fontSize="lg">
                        Make sure to copy your API token now. You won’t be able to see it again!
                    </Text>
                    <HStack>
                        <Code padding={5}>{props.token.token}</Code>
                        <Button onClick={onCopy} ml={2}>
                            {hasCopied ? "Copied" : "Copy"}
                        </Button>
                    </HStack>
                </VStack>
            }
        </Box>
    );
}

function Tokens(): JSX.Element {
    const { isOpen, onOpen, onClose } = useDisclosure();

    const [loading, setLoading] = useState<boolean>(true);
    const [tokens, setTokens] = useState<TokenType[]>([]);

    const [tokenName, setTokenName] = useState<string>("");
    const updateInputValue = useCallback((event: ChangeEvent<HTMLInputElement>) => {
        setTokenName(event.target.value);
    }, []);

    const createNewToken = useCallback(async () => {
        const token = randomstring.generate();
        const { data } = await supabaseClient
            .rpc("create_token", { name: tokenName, token })
            .single();

        if (data) {
            setTokens(ts => [
                { id: data["f1"], created_at: data["f2"], name: tokenName, token },
                ...ts,
            ]);
        }
        setTokenName("");
        onClose();
    }, [onClose, tokenName]);
    const revokeToken = useCallback((id: uuid) => {
        return async () => {
            await supabaseClient
                .from<TokenType>("tokens")
                .delete()
                .match({ id });

            // Remove the item
            setTokens(tokens.filter((t) => t.id !== id));
        };
    }, [tokens]);

    useEffect(() => {
        supabaseClient
            .from<TokenType>("tokens")
            .select("id, created_at, last_used_at, name")
            .order("created_at", { ascending: false })
            .then(({ data }) => {
                if (data) {
                    setLoading(false);
                    setTokens(data);
                }
            });
    }, []);

    if (loading) {
        return <Spinner />;
    }

    return (
        <VStack spacing={5}>
            <HStack>
                <Heading>API Tokens</Heading>
                <Spacer />
                <Tooltip
                    hasArrow
                    label="You cannot create tokens of more than 5!"
                    shouldWrapChildren
                    isDisabled={tokens.length < 5}
                >
                    <Button rightIcon={<AddIcon />} onClick={onOpen} isDisabled={tokens.length >= 5}>
                    New Token
                    </Button>
                </Tooltip>
            </HStack>
            <VStack>
                {tokens.map((t) => {
                    return <Token key={t.id} token={t} onRevoke={revokeToken(t.id)} />;
                })}
            </VStack>
            <Modal isOpen={isOpen} onClose={onClose}>
                <ModalOverlay />
                <ModalContent>
                    <ModalHeader>Create New Token</ModalHeader>
                    <ModalCloseButton />
                    <ModalBody>
                        <Input
                            placeholder="New token name"
                            onChange={updateInputValue}
                        />
                    </ModalBody>

                    <ModalFooter>
                        <Button variant="ghost" marginRight={3} onClick={onClose}>
                            Cancel
                        </Button>
                        <Button onClick={createNewToken} isDisabled={tokenName.length === 0}>
                            Create
                        </Button>
                    </ModalFooter>
                </ModalContent>
            </Modal>
        </VStack>
    );
}

interface ProfileProps {
    user: User;
}

function Profile(props: ProfileProps): JSX.Element {
    return (
        <VStack spacing={5}>
            <Heading>Profile Information</Heading>
            <HStack>
                <Avatar
                    size="2xl"
                    name={props.user.user_metadata["name"]}
                    src={props.user.user_metadata["avatar_url"]}
                />
                <Spacer />
                <VStack>
                    <HStack>
                        <Text as="b">Name</Text>
                        <Text>{props.user.user_metadata["name"]}</Text>
                    </HStack>
                    <HStack>
                        <Text as="b">GitHub Account</Text>
                        <Text>{props.user.user_metadata["user_name"]}</Text>
                    </HStack>
                    <HStack>
                        <Text as="b">User Email</Text>
                        <Text>{props.user.email}</Text>
                    </HStack>
                </VStack>
            </HStack>
        </VStack>
    );
}

interface SettingPageProps {
    initialTab: number;
    user: User;
}

function SettingPage(props: SettingPageProps): JSX.Element {
    const router = useRouter();

    const handleTabChange = useCallback((index: number): void => {
        router.push(`/settings/${tabs[index]}`, undefined, { shallow: true });
    }, [router]);

    return (
        <Center>
            <Tabs
                onChange={handleTabChange}
                defaultIndex={props.initialTab}
            >
                <TabList>
                    {tabs.map((label) => <Tab key={label}>{humanizeString(label)}</Tab>)}
                </TabList>
                <TabPanels>
                    <TabPanel>
                        <Profile user={props.user} />
                    </TabPanel>
                    <TabPanel>
                        <Tokens />
                    </TabPanel>
                </TabPanels>
            </Tabs>
        </Center>
    );
}

export default function Setting(props: SettingPageProps): JSX.Element {
    const { user } = useUser();

    return (
        <>
            <Meta title="Settings" />
            {user ? <SettingPage initialTab={props.initialTab} user={user} /> : <NeedAuth />}
        </>
    );
}

export const getServerSideProps: GetServerSideProps = async (context) => {
    const initialTab = tabs.indexOf(context.resolvedUrl.split("/").slice(-1)[0] as TabsType);
    if (initialTab === -1) {
        return {
            notFound: true,
        };
    }

    return {
        props: {
            initialTab,
        },
    };
};
