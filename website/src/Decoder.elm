module Decoder exposing (..)

import Json.Decode exposing (Decoder, bool, dict, field, int, list, map, map5, maybe, string, succeed)
import Json.Decode.Extra exposing (andMap)
import Model exposing (BuildMetadata, PackageMetadata, TestMetadata)



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
