module Decoder exposing (..)

import Model exposing (BuildMetadata, TestMetadata, PackageMetadata)
import Json.Decode exposing (Decoder, field, string, bool, int, list, dict, maybe, map, map5, succeed)
import Json.Decode.Extra exposing (andMap)



-- MODEL


buildDecoder : Decoder BuildMetadata
buildDecoder =
    map5 BuildMetadata
        (field "system" (maybe string))
        (field "bin" (maybe bool))
        (field "lib" (maybe bool))
        (field "compile_args" (maybe (list string)))
        (field "link_args" (maybe (list string)))


testDecoder : Decoder TestMetadata
testDecoder =
    map TestMetadata
        (field "framework" (maybe string))


packageDecoder : Decoder PackageMetadata
packageDecoder =
    succeed PackageMetadata
        |> andMap (field "cpp_version" int)
        |> andMap (field "owner" string)
        |> andMap (field "repo" string)
        |> andMap (field "version" string)
        |> andMap (field "description" string)
        |> andMap (field "dependencies" (maybe (dict string)))
        |> andMap (field "dev_dependencies" (maybe (dict string)))
        |> andMap (field "build_dependencies" (maybe (dict string)))
        |> andMap (field "build" (maybe buildDecoder))
        |> andMap (field "test" (maybe testDecoder))
        |> andMap (field "package_type" string)
        |> andMap (field "commit_sha" string)
        |> andMap (field "readme" string)
