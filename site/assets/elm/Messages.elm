module Messages exposing (..)

import Http
import Model exposing (ContactList, Contact)
import Navigation
import Routing exposing (Route)


type Msg
    = FetchResult (Result Http.Error ContactList)
    | Paginate Int
    | HandleSearchInput String
    | HandleFormSubmit
    | ResetSearch
    | UrlChange Navigation.Location
    | NavigateTo Route
    | FetchContactResult (Result Http.Error Contact)
