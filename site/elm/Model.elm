module Model exposing (..)

import Browser.Navigation exposing (Key)
import Dict exposing (Dict)
import Route exposing (Route)


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
    , readme : String
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
    , isChecked : Bool
    }
