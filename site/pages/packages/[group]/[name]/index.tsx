import { VStack, Heading, HStack, Tabs, TabList, Tab, TabPanels, TabPanel, Text, UnorderedList, ListItem, Link } from "@chakra-ui/react";
import type { GetServerSideProps } from "next";
import { supabaseServerClient } from "@supabase/supabase-auth-helpers/nextjs";

import type { Package as PackageType } from "../../../../types";

interface NameProps {
    package: PackageType;
    versions: string[];
    dependents: PackageType[];
}

export default function Name(props: NameProps): JSX.Element {
    return (
        <VStack spacing={5}>
            <HStack spacing={5}>
                <Heading>{props.package.name}</Heading>
                <Heading>v{props.package.version}</Heading>
            </HStack>
            <Heading size="md">
                {props.package.description}
            </Heading>

            <Tabs>
                <TabList>
                    <Tab>Readme</Tab>
                    <Tab>{props.versions.length} Versions</Tab>
                    <Tab>Dependencies</Tab>
                    <Tab>Dependents</Tab>
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
        </VStack>
    );
}

export const getServerSideProps: GetServerSideProps = async (context) => {
    const group = context.query.group;
    const name = context.query.name;

    const { data: packages } = await supabaseServerClient(context)
        .rpc<PackageType>("get_packages")
        .select("*") // TODO: Improve selection: name, total downloads, updated_at, ...
        .eq("name", `${group}/${name}`);

    if (packages && packages.length > 0) {
        const latestPackage = packages[0];
        // Retrieve dependents
        const { data: dependents } = await supabaseServerClient(context)
            .rpc<PackageType>("get_dependents", { "depname": latestPackage.name })
            .select("*");

        return {
            props: {
                package: latestPackage,
                versions: packages.map((p) => p.version),
                dependents,
            },
        };
    }
    return {
        notFound: true,
    };
};
