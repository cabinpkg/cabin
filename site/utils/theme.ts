import { extendTheme } from "@chakra-ui/react";
import type { StyleFunctionProps } from "@chakra-ui/theme-tools";
import { mode } from "@chakra-ui/theme-tools";
import type { Dict } from "@chakra-ui/utils";

const light = "#f5f5f5";
const dark = "#111111";

const theme = extendTheme({
    initialColorMode: "dark",
    useSystemColorMode: true,
    components: {
        Link: {
            baseStyle: (props: StyleFunctionProps | Dict) => ({
                color: mode("blue.500", "blue.200")(props),
            }),
        },
    },
    styles: {
        global: (props: StyleFunctionProps | Dict) => ({
            body: {
                bg: mode(light, dark)(props),
            },
        }),
    },
});

export default theme;
