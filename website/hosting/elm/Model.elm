module Model exposing (PackageMetadata, packageDecoder, IsFadein, Model, RemoteData(..), Flags)

import Browser.Navigation exposing (Key)
import Dict exposing (Dict)
import Route exposing (Route)
import Json.Decode as Decode
import Json.Decode.Extra exposing (andMap)


type RemoteData a
    = NotRequested
    | Requesting
    | Success a
    | Failure


type alias Flags =
    { api : String
    }


type alias PackageMetadata =
    { cppVersion : Int
    , owner : String
    , repo : String
    , version : String
    , description : String
    , dependencies : Maybe (Dict String String)
    , devDependencies : Maybe (Dict String String)
    , buildDependencies : Maybe (Dict String String)
    , build : Maybe BuildMetadata
    , test : Maybe TestMetadata
    , packageType : String
    , commitSha : String
    }


type alias BuildMetadata =
   { system : Maybe String
   , bin : Maybe Bool
   , lib : Maybe Bool
   , compileArgs : Maybe (List String)
   , linkArgs : Maybe (List String)
   }


type alias TestMetadata =
   { framework : Maybe String
   }


buildDecoder : Decode.Decoder BuildMetadata
buildDecoder =
    Decode.map5 BuildMetadata
        (Decode.field "system" (Decode.maybe Decode.string))
        (Decode.field "bin" (Decode.maybe Decode.bool))
        (Decode.field "lib" (Decode.maybe Decode.bool))
        (Decode.field "compile_args" (Decode.maybe (Decode.list Decode.string)))
        (Decode.field "link_args" (Decode.maybe (Decode.list Decode.string)))


testDecoder : Decode.Decoder TestMetadata
testDecoder =
    Decode.map TestMetadata
        (Decode.field "framework" (Decode.maybe Decode.string))


packageDecoder : Decode.Decoder PackageMetadata
packageDecoder =
    Decode.succeed PackageMetadata
        |> andMap (Decode.field "cpp_version" Decode.int)
        |> andMap (Decode.field "owner" Decode.string)
        |> andMap (Decode.field "repo" Decode.string)
        |> andMap (Decode.field "version" Decode.string)
        |> andMap (Decode.field "description" Decode.string)
        |> andMap (Decode.field "dependencies" (Decode.maybe (Decode.dict Decode.string)))
        |> andMap (Decode.field "dev_dependencies" (Decode.maybe (Decode.dict Decode.string)))
        |> andMap (Decode.field "build_dependencies" (Decode.maybe (Decode.dict Decode.string)))
        |> andMap (Decode.field "build" (Decode.maybe buildDecoder))
        |> andMap (Decode.field "test" (Decode.maybe testDecoder))
        |> andMap (Decode.field "package_type" Decode.string)
        |> andMap (Decode.field "commit_sha" Decode.string)


type alias IsFadein =
    { abstract : Bool
    , section1 : Bool
    , demo : Bool
    , getStart : Bool
    }


type alias Model =
    { flags : Flags
    , navKey : Key
    , route : Route
    , ownPackages : RemoteData (List (RemoteData PackageMetadata))
    , packageVersions : RemoteData (List String)
    , package : RemoteData PackageMetadata
    , search : String
    , isFadein : IsFadein
    , searchInput : String
    , width : Int
    , isChecked : Bool
    , readme : Maybe String
    }
