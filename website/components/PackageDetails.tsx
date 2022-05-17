import {
    Avatar,
    Code, Divider,
    Heading,
    HStack, Link, ListItem,
    Tab,
    TabList,
    TabPanel,
    TabPanels,
    Tabs, Tag,
    Text,
    UnorderedList,
    VStack,
} from "@chakra-ui/react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { faArrowUpRightFromSquare, faFileLines, faLink, faTags } from "@fortawesome/free-solid-svg-icons";
import { useEffect, useState } from "react";
import { supabaseClient } from "@supabase/supabase-auth-helpers/nextjs";

import type { Package as PackageType, User } from "~/utils/types";

interface PackageSubProps {
    package: PackageType;
}

function PackageSub(props: PackageSubProps): JSX.Element {
    const [owners, setOwners] = useState<User[]>([]);

    useEffect(() => {
        supabaseClient
            .rpc<User>("get_owners", { pkgname: props.package.name })
            .select("*")
            .then(({ data }) => {
                if (data) {
                    setOwners(data);
                }
            });
    }, [props.package.name]);

    return (
        <VStack spacing={5}>
            <Text as="b">Owners</Text>
            <Divider />
            {owners.map((o) =>
                <HStack key={o.id} spacing={1}>
                    <Avatar size="xs" name={o.name} src={o.avatar_url} />
                    <Link href={`/packages/${o.user_name}`}>{o.name}</Link>
                </HStack>
            )}
        </VStack>
    );
}

interface PackageMainProps {
    package: PackageType;
    versions: string[];
    dependents: PackageType[];
}

function PackageMain(props: PackageMainProps): JSX.Element {
    return (
        <Tabs>
            <TabList>
                <Tab>
                    <HStack spacing={2}>
                        <FontAwesomeIcon icon={faFileLines} width={13} />
                        <Text>Readme</Text>
                    </HStack>
                </Tab>
                <Tab>
                    <HStack spacing={1}>
                        <FontAwesomeIcon icon={faTags} width={18} />
                        <Code>{props.versions.length}</Code>
                        <Text>Versions</Text>
                    </HStack>
                </Tab>
                <Tab>
                    <HStack spacing={1}>
                        <FontAwesomeIcon icon={faArrowUpRightFromSquare} width={15} />
                        {props.package.metadata["dependencies"] ?
                            // eslint-disable-next-line @typescript-eslint/ban-ts-comment
                            // @ts-ignore
                            <Code>{Object.keys(props.package.metadata["dependencies"]).length}</Code> :
                            <Code>0</Code>
                        }
                        <Text>Dependencies</Text>
                    </HStack>
                </Tab>
                <Tab>
                    <HStack spacing={1}>
                        <FontAwesomeIcon icon={faLink} width={20} />
                        <Code>{props.dependents.length}</Code>
                        <Text>Dependents</Text>
                    </HStack>
                </Tab>
            </TabList>

            <TabPanels>
                <TabPanel>
                    <Text>no readme found</Text>
                </TabPanel>
                <TabPanel>
                    <UnorderedList>
                        {props.versions.map((v) => <ListItem key={v}>
                            <Link href={`/packages/${props.package.name}/${v}`}>{v}</Link>
                        </ListItem>)}
                    </UnorderedList>
                </TabPanel>
                <TabPanel>
                    {props.package.metadata["dependencies"] ?
                        <UnorderedList>
                            {/* eslint-disable-next-line @typescript-eslint/ban-ts-comment */}
                            {/* @ts-ignore */}
                            {Object.entries(props.package.metadata["dependencies"]).map(([name, ver]) => <ListItem key={name}>{name}: {ver}</ListItem>)}
                        </UnorderedList> :
                        <Text>This package has no dependencies.</Text>
                    }
                </TabPanel>
                <TabPanel>
                    {props.dependents && props.dependents.length > 0 ?
                        <UnorderedList>
                            {props.dependents.map((d) => <ListItem key={d.id}>{d.name}: {d.version}</ListItem>)}
                        </UnorderedList> :
                        <Text>This package is not used as a dependency yet.</Text>
                    }
                </TabPanel>
            </TabPanels>
        </Tabs>
    );
}

interface PackageHeadingProps {
    package: PackageType;
}

function PackageHeading(props: PackageHeadingProps): JSX.Element {
    return (
        <VStack spacing={5}>
            <HStack spacing={5}>
                <Heading>{props.package.name}</Heading>
                <Heading>v{props.package.version}</Heading>
                <Tag>C++{props.package.edition.toString().slice(-2)}</Tag>
            </HStack>
            <Heading size="md">
                {props.package.description}
            </Heading>
        </VStack>
    );
}

interface PackageDetailsProps {
    package: PackageType;
    versions: string[];
    dependents: PackageType[];
}

export default function PackageDetails(props: PackageDetailsProps): JSX.Element {
    return (
        <VStack spacing={5}>
            <PackageHeading package={props.package} />
            <HStack spacing={5}>
                <PackageMain package={props.package} versions={props.versions} dependents={props.dependents} />
                <PackageSub package={props.package} />
            </HStack>
        </VStack>
    );
}
