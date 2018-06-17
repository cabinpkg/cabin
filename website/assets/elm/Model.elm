module Model exposing (..)

import Routing exposing (Route)


type RemoteData e a
    = NotRequested
    | Requesting
    | Failure e
    | Success a

type alias Model =
    { contactList : RemoteData String ContactList
    , search : String
    , route : Route
    , contact : RemoteData String Contact
    , searchList : RemoteData String SearchList
    }

type alias ContactList =
    { entries : List Contact
    , page_number : Int
    , total_entries : Int
    , total_pages : Int
    }

type alias Contact =
    { id : Int
    , first_name : String
    , last_name : String
    , gender : Int
    , birth_date : String
    , location : String
    , phone_number : String
    , email : String
    , headline : String
    , picture : String
    }

type alias SearchList =
    { word : List String }

initialSearchList : SearchList
initialSearchList =
    { word = [] }

initialContactList : ContactList
initialContactList =
    { entries = []
    , page_number = 1
    , total_entries = 0
    , total_pages = 0
    }

initialModel : Route -> Model
initialModel route =
    { contactList = NotRequested
    , search = ""
    , route = route
    , contact = NotRequested
    , searchList = NotRequested
    }
