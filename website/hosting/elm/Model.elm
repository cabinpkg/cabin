module Model exposing (Dependency, DetailedPackage, IsFadein, Links, Model, Package, RemoteData(..), SigninUser, Token, User, Flags)

import Routing exposing (Route)
import Browser.Navigation exposing (Key)


type RemoteData a
    = NotRequested
    | Requesting
    | Success a
    | Failure


type alias Flags =
    { api : String
    }

type alias Token =
    { id : String
    , name : String
    , owner : String
    , created_date : String
    , last_used_date : Maybe String
    }


type alias SigninUser =
    { name : String
    , photo_url : String
    }


type alias User =
    { id : String
    , name : String
    , photo_url : String
    , github_link : String
    }


type alias Package =
    { name : String
    , version : String -- newest
    , owners : List String
    , cpp_version : Int
    , description : String
    }


type alias Dependency =
    { name : String
    , version : String
    }


type alias Links =
    { github : Maybe String
    , homepage : Maybe String
    }


type alias DetailedPackage =
    { name : String
    , versions : List String
    , owners : List String
    , cpp_version : Int
    , description : String
    , deps : Maybe (List Dependency)
    , md5hash : String
    , links : Maybe Links
    , license : Maybe String
    , created_date : String
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
    , signinUser : RemoteData SigninUser
    , signinId : String
    , otherUser : RemoteData User
    , currentToken : RemoteData (List Token)
    , listPackages : RemoteData (List Package)
    , detailedPackage : RemoteData DetailedPackage
    , search : String
    , newTokenName : String
    , isFadein : IsFadein
    , searchInput : String
    , width : Int
    , isChecked : Bool
    }
